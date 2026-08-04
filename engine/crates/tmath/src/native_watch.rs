//! Event-driven native rendering for `tmath watch <file>`.

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, SystemTime};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use tmath_core::ipc::IPC_MAX_REQUEST_BYTES;
use tmath_core::terminal::{StdioTty, Terminal};
use tmath_render::{
    CacheBudget, Limits, PlacementPlanner, RenderCache, RenderError, RenderOptions, Revision,
    SafeErrorDetails, SafeErrorRecord, SafeLimitKind, StreamSplitter,
};

use crate::native_stream::{self, StreamSink};

enum WaitMode {
    Native {
        _watcher: RecommendedWatcher,
        receiver: Receiver<WatchMessage>,
    },
    Poll {
        interval: Duration,
        stamp: FileStamp,
        exit: Receiver<()>,
    },
}

enum WatchMessage {
    File(notify::Result<Event>),
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileStamp {
    present: bool,
    modified: Option<SystemTime>,
    len: u64,
}

pub(crate) fn run(
    path: &Path,
    content_width: Option<u32>,
    font_size: Option<u32>,
    poll_ms: u64,
    connected: Option<(Terminal<StdioTty>, (u32, u32))>,
) -> Result<i32, String> {
    let path = absolute_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| "watched file has no parent directory".to_string())?
        .to_path_buf();
    // Auto-fit to the connected terminal's pane when no explicit override was
    // given; the non-terminal path (`connected` is `None`) keeps the fixed
    // defaults, which is what the hermetic tests for this path drive.
    let fitted = crate::layout::fitted_layout_for_connected(&connected);
    let device_pixel_ratio = crate::layout::resolve_device_pixel_ratio(fitted);
    let options = RenderOptions::new(
        crate::layout::resolve_content_width_pt(content_width, fitted),
        crate::layout::resolve_font_size_pt(font_size, fitted),
        device_pixel_ratio,
    )
    .map_err(|_| "invalid native watch render options".to_string())?;
    let limits = Limits::default();
    let scaled = limits.scaled(device_pixel_ratio);
    let max_entries = usize::try_from(limits.blocks_per_document)
        .unwrap_or(usize::MAX)
        .max(1);
    let mut cache = RenderCache::new(CacheBudget {
        max_entries,
        max_pixels: scaled.image_pixels.max(1),
    });
    let mut planner = PlacementPlanner::new();
    let mut formula_errors = Vec::new();
    let interactive = connected.is_some();
    let mut sink = StreamSink::new(connected, scaled.image_pixels);
    // Arm the directory watch before the initial read/render so a save that
    // lands immediately after the initial events cannot fall into a setup gap.
    let mut mode = create_wait_mode(&parent, &path, poll_ms, interactive);

    let initial = match read_revision(&path, limits) {
        Ok(revision) => revision,
        Err(ReadRevisionError::Missing) => {
            return Err("watched file does not exist for the initial render".into())
        }
        Err(ReadRevisionError::Render(error)) => {
            eprintln!("{}", safe_json(&error));
            return Ok(1);
        }
    };
    if let Err(error) = native_stream::apply_revision(
        &initial,
        &options,
        &mut cache,
        &mut planner,
        &mut formula_errors,
        &mut sink,
    ) {
        eprintln!("{}", safe_json(&error));
        return Ok(1);
    }

    let mut waiting = false;
    let mut error_reported = false;
    loop {
        let changed = match &mut mode {
            WaitMode::Native {
                receiver,
                _watcher: _,
            } => match wait_native(receiver, &path)? {
                WaitOutcome::Changed => true,
                WaitOutcome::Unrelated => false,
                WaitOutcome::Exit => {
                    sink.finish().map_err(|error| safe_json(&error))?;
                    return Ok(0);
                }
            },
            WaitMode::Poll {
                interval,
                stamp,
                exit,
            } => match wait_poll(*interval, stamp, &path, exit) {
                WaitOutcome::Changed => true,
                WaitOutcome::Unrelated => false,
                WaitOutcome::Exit => {
                    sink.finish().map_err(|error| safe_json(&error))?;
                    return Ok(0);
                }
            },
        };
        if !changed {
            continue;
        }

        match read_revision(&path, limits) {
            Ok(revision) => {
                waiting = false;
                error_reported = false;
                if let Err(error) = native_stream::apply_revision(
                    &revision,
                    &options,
                    &mut cache,
                    &mut planner,
                    &mut formula_errors,
                    &mut sink,
                ) {
                    report_revision_error(&mut sink, &error)?;
                }
            }
            Err(ReadRevisionError::Missing) => {
                error_reported = false;
                if !waiting {
                    sink.summary_event("event=waiting")
                        .map_err(|error| safe_json(&error))?;
                    waiting = true;
                }
            }
            Err(ReadRevisionError::Render(error)) => {
                waiting = false;
                if !error_reported {
                    report_revision_error(&mut sink, &error)?;
                    error_reported = true;
                }
            }
        }
    }
}

fn create_wait_mode(parent: &Path, path: &Path, poll_ms: u64, interactive: bool) -> WaitMode {
    let (sender, receiver) = mpsc::channel();
    let event_sender = sender.clone();
    let watcher = notify::recommended_watcher(move |event| {
        let _ = event_sender.send(WatchMessage::File(event));
    })
    .and_then(|mut watcher| {
        watcher.watch(parent, RecursiveMode::NonRecursive)?;
        Ok(watcher)
    });
    match watcher {
        Ok(watcher) => {
            if interactive {
                spawn_control_reader(sender);
            }
            WaitMode::Native {
                _watcher: watcher,
                receiver,
            }
        }
        Err(error) => {
            eprintln!(
                "tmath: native file watcher unavailable ({error}); falling back to --poll-ms {poll_ms}"
            );
            let (exit_sender, exit) = mpsc::channel();
            if interactive {
                spawn_poll_control_reader(exit_sender);
            }
            WaitMode::Poll {
                interval: Duration::from_millis(poll_ms),
                stamp: file_stamp(path),
                exit,
            }
        }
    }
}

enum WaitOutcome {
    Changed,
    Unrelated,
    Exit,
}

fn wait_native(receiver: &Receiver<WatchMessage>, target: &Path) -> Result<WaitOutcome, String> {
    let first = receiver
        .recv()
        .map_err(|_| "native file watcher channel disconnected".to_string())?;
    let mut relevant = match first {
        WatchMessage::File(event) => event_is_relevant(event, target),
        WatchMessage::Exit => return Ok(WaitOutcome::Exit),
    };
    loop {
        match receiver.try_recv() {
            Ok(WatchMessage::File(event)) => relevant |= event_is_relevant(event, target),
            Ok(WatchMessage::Exit) => return Ok(WaitOutcome::Exit),
            Err(TryRecvError::Empty) => {
                return Ok(if relevant {
                    WaitOutcome::Changed
                } else {
                    WaitOutcome::Unrelated
                })
            }
            Err(TryRecvError::Disconnected) => {
                return Err("native file watcher channel disconnected".into())
            }
        }
    }
}

fn event_is_relevant(event: notify::Result<Event>, target: &Path) -> bool {
    let Ok(event) = event else {
        return false;
    };
    let parent = target.parent();
    !event.kind.is_access()
        && event.paths.iter().any(|path| {
            paths_match(path, target) || parent.is_some_and(|parent| paths_match(path, parent))
        })
}

fn paths_match(candidate: &Path, target: &Path) -> bool {
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(candidate)
    };
    candidate == target
}

fn wait_poll(
    interval: Duration,
    stamp: &mut FileStamp,
    path: &Path,
    exit: &Receiver<()>,
) -> WaitOutcome {
    if exit.recv_timeout(interval).is_ok() {
        return WaitOutcome::Exit;
    }
    let next = file_stamp(path);
    if next == *stamp {
        return WaitOutcome::Unrelated;
    }
    *stamp = next;
    WaitOutcome::Changed
}

fn spawn_control_reader(sender: mpsc::Sender<WatchMessage>) {
    std::thread::spawn(move || {
        if control_reader_exited() {
            let _ = sender.send(WatchMessage::Exit);
        }
    });
}

fn spawn_poll_control_reader(sender: mpsc::Sender<()>) {
    std::thread::spawn(move || {
        if control_reader_exited() {
            let _ = sender.send(());
        }
    });
}

fn control_reader_exited() -> bool {
    use tmath_core::input::InputDecoder;
    use tmath_core::scroll_driver::is_exit_signal;

    let Ok(mut tty) = File::open("/dev/tty") else {
        return false;
    };
    let mut decoder = InputDecoder::new();
    let mut bytes = [0; 64];
    loop {
        match tty.read(&mut bytes) {
            Ok(0) => return false,
            Ok(count) => {
                decoder.push(&bytes[..count]);
                while let Some(event) = decoder.next_event() {
                    if is_exit_signal(&event) {
                        return true;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
}

fn file_stamp(path: &Path) -> FileStamp {
    match fs::metadata(path) {
        Ok(metadata) => FileStamp {
            present: true,
            modified: metadata.modified().ok(),
            len: metadata.len(),
        },
        Err(_) => FileStamp {
            present: false,
            modified: None,
            len: 0,
        },
    }
}

enum ReadRevisionError {
    Missing,
    Render(RenderError),
}

fn read_revision(path: &Path, limits: Limits) -> Result<Revision, ReadRevisionError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ReadRevisionError::Missing)
        }
        Err(_) => return Err(ReadRevisionError::Render(read_error())),
    };
    let mut bytes = Vec::new();
    file.by_ref()
        .take((IPC_MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadRevisionError::Render(read_error()))?;
    if bytes.len() > IPC_MAX_REQUEST_BYTES {
        return Err(ReadRevisionError::Render(input_limit_error(bytes.len())));
    }
    let mut splitter = StreamSplitter::new(limits);
    splitter
        .push(&bytes)
        .and_then(|_| splitter.finish())
        .map_err(ReadRevisionError::Render)
}

fn input_limit_error(actual: usize) -> RenderError {
    RenderError::new(
        SafeErrorRecord {
            code: tmath_render::ErrorCode::RendererInputLimit,
            retryable: false,
            details: Some(SafeErrorDetails {
                limit_kind: Some(SafeLimitKind::InputBytes),
                limit: Some(IPC_MAX_REQUEST_BYTES as u64),
                actual: Some(actual as u64),
                ..SafeErrorDetails::default()
            }),
        },
        "watched document exceeds input limit",
    )
}

fn read_error() -> RenderError {
    RenderError::new(
        SafeErrorRecord {
            code: tmath_render::ErrorCode::RendererFailed,
            retryable: true,
            details: None,
        },
        "watched document read failed",
    )
}

fn report_revision_error(sink: &mut StreamSink, error: &RenderError) -> Result<(), String> {
    eprintln!("{}", safe_json(error));
    let code = serde_json::to_value(error.safe_record())
        .ok()
        .and_then(|record| {
            record
                .get("code")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "internal_error".to_string());
    sink.summary_event(&format!("event=error code={code}"))
        .map_err(|sink_error| safe_json(&sink_error))
}

fn safe_json(error: &RenderError) -> String {
    serde_json::to_string(error.safe_record())
        .unwrap_or_else(|_| r#"{"code":"internal_error","retryable":false}"#.to_string())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| format!("resolve watched file path: {error}"))
    }
}
