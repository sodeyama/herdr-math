//! Incremental native rendering for `tmath render --engine native -`.

use std::io::{self, Read as _, Write as _};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use tmath_core::placement::{
    decode_png, emit_placed_block_cursor, CellSize, PlacementLimits, TerminalOp,
};
use tmath_core::terminal::{StdioTty, Terminal};
use tmath_render::{
    content_hash, CacheBudget, Limits, PlacementPlanner, Plan, PlanOp, RenderCache, RenderError,
    RenderOptions, RenderedBlock, Revision, SafeErrorRecord, StreamSplitter,
};

use crate::terminal_output;

const READ_CHUNK_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheOutcome {
    Hit,
    Miss,
}

impl CacheOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
        }
    }
}

struct PreparedBlock {
    rendered: Option<Arc<RenderedBlock>>,
    png: Option<Vec<u8>>,
    cache: Option<CacheOutcome>,
}

enum InputEvent {
    Chunk(Vec<u8>),
    Eof,
    Failed,
}

pub(crate) fn run(
    content_width: Option<u32>,
    font_size: Option<u32>,
    connected: Option<(Terminal<StdioTty>, (u32, u32))>,
) -> Result<(), RenderError> {
    // Auto-fit to the connected terminal's pane when no explicit override was
    // given; the non-terminal path (`connected` is `None`, what the hermetic
    // summary/event-line tests drive) keeps the fixed defaults.
    let fitted = crate::layout::fitted_layout_for_connected(&connected);
    let device_pixel_ratio = crate::layout::resolve_device_pixel_ratio(fitted);
    let options = RenderOptions::new(
        crate::layout::resolve_content_width_pt(content_width, fitted),
        crate::layout::resolve_font_size_pt(font_size, fitted),
        device_pixel_ratio,
    )
    .map_err(|_| stream_error())?;
    let limits = Limits::default();
    let scaled = limits.scaled(device_pixel_ratio);
    let max_entries = usize::try_from(limits.blocks_per_document)
        .unwrap_or(usize::MAX)
        .max(1);
    let mut cache = RenderCache::new(CacheBudget {
        max_entries,
        max_pixels: scaled.image_pixels.max(1),
    });
    let mut splitter = StreamSplitter::new(limits);
    let mut planner = PlacementPlanner::new();
    let mut formula_errors = Vec::new();
    let mut sink = StreamSink::new(connected, scaled.image_pixels);
    let receiver = spawn_reader();
    let mut eof = false;

    while !eof {
        let event = receiver.recv().map_err(|_| stream_error())?;
        match event {
            InputEvent::Chunk(first) => {
                let mut bytes = first;
                loop {
                    match receiver.try_recv() {
                        Ok(InputEvent::Chunk(chunk)) => bytes.extend_from_slice(&chunk),
                        Ok(InputEvent::Eof) => {
                            eof = true;
                            break;
                        }
                        Ok(InputEvent::Failed) => return Err(stream_error()),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return Err(stream_error()),
                    }
                }

                // One drained batch produces one revision. Completed blocks
                // emit immediately, while all queued tail bytes collapse into
                // the newest tail rendered once in this loop iteration.
                let revision = splitter.push(&bytes)?;
                apply_revision(
                    &revision,
                    &options,
                    &mut cache,
                    &mut planner,
                    &mut formula_errors,
                    &mut sink,
                )?;
            }
            InputEvent::Eof => eof = true,
            InputEvent::Failed => return Err(stream_error()),
        }
    }

    let revision = splitter.finish()?;
    apply_revision(
        &revision,
        &options,
        &mut cache,
        &mut planner,
        &mut formula_errors,
        &mut sink,
    )?;
    sink.done(planner.blocks().len(), formula_errors.iter().sum())?;
    Ok(())
}

fn spawn_reader() -> Receiver<InputEvent> {
    // Keep at most one unread chunk in userspace. Bytes arriving while a tail
    // render is in flight remain in the pipe and are consumed as the newest
    // available content on the next loop iteration.
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        loop {
            let mut chunk = vec![0; READ_CHUNK_BYTES];
            match stdin.read(&mut chunk) {
                Ok(0) => {
                    let _ = sender.send(InputEvent::Eof);
                    return;
                }
                Ok(count) => {
                    chunk.truncate(count);
                    if sender.send(InputEvent::Chunk(chunk)).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    let _ = sender.send(InputEvent::Failed);
                    return;
                }
            }
        }
    });
    receiver
}

pub(crate) fn apply_revision(
    revision: &Revision,
    options: &RenderOptions,
    cache: &mut RenderCache,
    planner: &mut PlacementPlanner,
    formula_errors: &mut Vec<usize>,
    sink: &mut StreamSink,
) -> Result<(), RenderError> {
    let previous = planner.blocks().to_vec();
    let previous_formula_errors = formula_errors.clone();
    let mut inputs = Vec::with_capacity(revision.blocks.len());
    let mut prepared = Vec::with_capacity(revision.blocks.len());
    let mut next_formula_errors = Vec::with_capacity(revision.blocks.len());

    for (index, block) in revision.blocks.iter().enumerate() {
        let hash = content_hash(block, options);
        let unchanged = previous
            .get(index)
            .is_some_and(|planned| planned.hash == hash);
        if unchanged {
            let planned = &previous[index];
            inputs.push((hash, planned.width_px, planned.height_px));
            prepared.push(PreparedBlock {
                rendered: None,
                png: None,
                cache: None,
            });
            next_formula_errors.push(previous_formula_errors[index]);
            continue;
        }

        let before = cache.stats();
        let rendered = cache.render(block, options)?;
        let after = cache.stats();
        let outcome = if after.hits > before.hits {
            CacheOutcome::Hit
        } else {
            CacheOutcome::Miss
        };
        inputs.push((hash, rendered.width_px, rendered.height_px));
        next_formula_errors.push(rendered.formula_errors.len());
        let png = crate::native_render::canonical_block_png(
            &rendered,
            Limits::default()
                .scaled(options.device_pixel_ratio)
                .image_pixels,
        )?;
        prepared.push(PreparedBlock {
            rendered: Some(rendered),
            png: Some(png),
            cache: Some(outcome),
        });
    }

    let plan = planner.plan(&inputs);
    sink.emit(&plan, &prepared, &previous)?;
    *formula_errors = next_formula_errors;
    Ok(())
}

pub(crate) enum StreamSink {
    Summary,
    Terminal(TerminalSink),
}

impl StreamSink {
    pub(crate) fn new(
        connected: Option<(Terminal<StdioTty>, (u32, u32))>,
        max_image_pixels: u64,
    ) -> Self {
        match connected {
            Some((terminal, cell)) => Self::Terminal(TerminalSink::new(
                terminal,
                CellSize {
                    width: cell.0,
                    height: cell.1,
                },
                max_image_pixels,
            )),
            None => Self::Summary,
        }
    }

    fn emit(
        &mut self,
        plan: &Plan,
        prepared: &[PreparedBlock],
        previous: &[tmath_render::PlannedBlock],
    ) -> Result<(), RenderError> {
        match self {
            Self::Summary => emit_summary(plan, prepared),
            Self::Terminal(sink) => sink.emit(plan, prepared, previous),
        }
    }

    pub(crate) fn finish(&mut self) -> Result<(), RenderError> {
        match self {
            Self::Summary => Ok(()),
            Self::Terminal(sink) => sink.finish(),
        }
    }

    pub(crate) fn done(&mut self, blocks: usize, formula_errors: usize) -> Result<(), RenderError> {
        match self {
            Self::Summary => {
                println!("event=done blocks={blocks} formula_errors={formula_errors}");
                io::stdout().flush().map_err(|_| stream_error())
            }
            Self::Terminal(_) => self.finish(),
        }
    }

    pub(crate) fn summary_event(&mut self, event: &str) -> Result<(), RenderError> {
        if matches!(self, Self::Summary) {
            println!("{event}");
            io::stdout().flush().map_err(|_| stream_error())?;
        }
        Ok(())
    }
}

fn emit_summary(plan: &Plan, prepared: &[PreparedBlock]) -> Result<(), RenderError> {
    for (index, operation) in plan.ops.iter().enumerate() {
        match operation {
            PlanOp::Keep { .. } => {}
            PlanOp::Append { block } => {
                let (_, png, cache) = rendered_event(prepared, index)?;
                println!(
                    "event=append id={} width={} height={} bytes={} cache={}",
                    block.id,
                    block.width_px,
                    block.height_px,
                    png.len(),
                    cache.label()
                );
                io::stdout().flush().map_err(|_| stream_error())?;
            }
            PlanOp::Replace { old_id, block } => {
                let (_, png, cache) = rendered_event(prepared, index)?;
                println!(
                    "event=replace old={} id={} width={} height={} bytes={} cache={}",
                    old_id,
                    block.id,
                    block.width_px,
                    block.height_px,
                    png.len(),
                    cache.label()
                );
                io::stdout().flush().map_err(|_| stream_error())?;
            }
            PlanOp::Remove { id } => {
                println!("event=remove id={id}");
                io::stdout().flush().map_err(|_| stream_error())?;
            }
        }
    }
    Ok(())
}

fn rendered_event(
    prepared: &[PreparedBlock],
    index: usize,
) -> Result<(&RenderedBlock, &[u8], CacheOutcome), RenderError> {
    let prepared = prepared.get(index).ok_or_else(stream_error)?;
    let rendered = prepared.rendered.as_deref().ok_or_else(stream_error)?;
    let png = prepared.png.as_deref().ok_or_else(stream_error)?;
    let cache = prepared.cache.ok_or_else(stream_error)?;
    Ok((rendered, png, cache))
}

#[derive(Clone, Copy)]
struct PlacedState {
    id: u64,
    rows: u32,
    pixels: u64,
}

pub(crate) struct TerminalSink {
    terminal: Terminal<StdioTty>,
    cell: CellSize,
    max_image_pixels: u64,
    placement_limits: PlacementLimits,
    placed: Vec<PlacedState>,
    first_append_at_line_start: bool,
}

impl TerminalSink {
    fn new(mut terminal: Terminal<StdioTty>, cell: CellSize, max_image_pixels: u64) -> Self {
        let first_append_at_line_start = if tmath_core::kitty::inside_tmux() {
            false
        } else {
            terminal.cursor_column().ok().flatten() == Some(1)
        };
        Self {
            terminal,
            cell,
            max_image_pixels,
            placement_limits: PlacementLimits::default(),
            placed: Vec::new(),
            first_append_at_line_start,
        }
    }

    fn emit(
        &mut self,
        plan: &Plan,
        prepared: &[PreparedBlock],
        previous: &[tmath_render::PlannedBlock],
    ) -> Result<(), RenderError> {
        for (index, operation) in plan.ops.iter().enumerate() {
            match operation {
                PlanOp::Keep { .. } => {}
                PlanOp::Append { block } => {
                    let (rendered, png, _) = rendered_event(prepared, index)?;
                    self.append(block.id, rendered, png)?;
                }
                PlanOp::Replace { old_id, block } => {
                    let (rendered, png, _) = rendered_event(prepared, index)?;
                    let old_index = previous
                        .iter()
                        .position(|previous| previous.id == *old_id)
                        .ok_or_else(stream_error)?;
                    self.replace(
                        *old_id,
                        block.id,
                        rendered,
                        png,
                        old_index + 1 == previous.len(),
                    )?;
                }
                PlanOp::Remove { id } => self.remove(*id)?,
            }
        }
        Ok(())
    }

    fn append(&mut self, id: u64, rendered: &RenderedBlock, png: &[u8]) -> Result<(), RenderError> {
        let decoded = self.decode(id, rendered, png)?;
        self.validate_placement(decoded.pixels, None)?;
        let operations = append_operations(
            decoded.id,
            rendered.width_px,
            rendered.height_px,
            &decoded.rgba,
            decoded.cols,
            decoded.rows,
            self.first_append_at_line_start || !self.placed.is_empty(),
        );
        terminal_output::write_operations(&operations).map_err(|_| stream_error())?;
        self.first_append_at_line_start = true;
        self.placed.push(PlacedState {
            id,
            rows: decoded.rows,
            pixels: decoded.pixels,
        });
        Ok(())
    }

    fn replace(
        &mut self,
        old_id: u64,
        new_id: u64,
        rendered: &RenderedBlock,
        png: &[u8],
        was_last: bool,
    ) -> Result<(), RenderError> {
        let old_index = self
            .placed
            .iter()
            .position(|placed| placed.id == old_id)
            .ok_or_else(stream_error)?;
        let old = self.placed[old_index];
        let decoded = self.decode(new_id, rendered, png)?;
        self.validate_placement(decoded.pixels, Some(old_index))?;
        let top_is_reachable = was_last
            && self.placed.last().is_some_and(|placed| placed.id == old.id)
            && self
                .terminal
                .cursor_position()
                .ok()
                .flatten()
                .is_some_and(|(row, _)| row > old.rows);

        let operations = if top_is_reachable {
            tail_replace_operations(TailReplace {
                old_image_id: old.id,
                new_image_id: decoded.id,
                width_px: rendered.width_px,
                height_px: rendered.height_px,
                rgba: &decoded.rgba,
                cols: decoded.cols,
                old_rows: old.rows,
                new_rows: decoded.rows,
            })?
        } else {
            // Phase 2 intentionally cannot re-anchor an interior block or a
            // tail whose top row has entered scrollback. Delete only that image
            // (leaving its old cells blank) and append the replacement at the
            // bottom. In-place interior re-anchoring belongs to Phase 3 viewer
            // work, where viewport and history state are explicit.
            let mut operations = vec![TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(
                u32::try_from(old.id).map_err(|_| stream_error())?,
            ))];
            operations.extend(append_operations(
                decoded.id,
                rendered.width_px,
                rendered.height_px,
                &decoded.rgba,
                decoded.cols,
                decoded.rows,
                true,
            ));
            operations
        };
        terminal_output::write_operations(&operations).map_err(|_| stream_error())?;
        self.placed.remove(old_index);
        self.placed.push(PlacedState {
            id: new_id,
            rows: decoded.rows,
            pixels: decoded.pixels,
        });
        Ok(())
    }

    fn remove(&mut self, id: u64) -> Result<(), RenderError> {
        let index = self
            .placed
            .iter()
            .position(|placed| placed.id == id)
            .ok_or_else(stream_error)?;
        let image_id = u32::try_from(id).map_err(|_| stream_error())?;
        terminal_output::write_operations(&[TerminalOp::Graphics(
            tmath_core::kitty::kitty_delete_id(image_id),
        )])
        .map_err(|_| stream_error())?;
        self.placed.remove(index);
        Ok(())
    }

    fn decode(
        &self,
        id: u64,
        rendered: &RenderedBlock,
        png: &[u8],
    ) -> Result<DecodedPlacement, RenderError> {
        let id = u32::try_from(id).map_err(|_| stream_error())?;
        let (width, height, rgba) =
            decode_png(png, self.max_image_pixels).map_err(|_| stream_error())?;
        if width != rendered.width_px || height != rendered.height_px {
            return Err(stream_error());
        }
        let (cols, rows) = tmath_core::placement::grid_for(width, height, self.cell);
        Ok(DecodedPlacement {
            id,
            cols,
            rows,
            pixels: u64::from(width) * u64::from(height),
            rgba,
        })
    }

    fn validate_placement(
        &self,
        new_pixels: u64,
        replacing: Option<usize>,
    ) -> Result<(), RenderError> {
        let count = self
            .placed
            .len()
            .saturating_add(usize::from(replacing.is_none()));
        if count > self.placement_limits.max_concurrent_placements {
            return Err(stream_error());
        }
        let pixels = self
            .placed
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != replacing)
            .map(|(_, placed)| placed.pixels)
            .sum::<u64>()
            .saturating_add(new_pixels);
        if pixels > self.placement_limits.max_total_pixels {
            return Err(stream_error());
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), RenderError> {
        self.terminal.reset().map_err(|_| stream_error())?;
        println!();
        Ok(())
    }
}

struct DecodedPlacement {
    id: u32,
    cols: u32,
    rows: u32,
    pixels: u64,
    rgba: Vec<u8>,
}

fn append_operations(
    image_id: u32,
    width_px: u32,
    height_px: u32,
    rgba: &[u8],
    cols: u32,
    rows: u32,
    already_at_line_start: bool,
) -> Vec<TerminalOp> {
    let mut operations = emit_placed_block_cursor(
        image_id,
        width_px,
        height_px,
        rgba,
        cols,
        rows,
        already_at_line_start,
    );
    operations.push(TerminalOp::Local(b"\r\n".to_vec()));
    operations
}

struct TailReplace<'a> {
    old_image_id: u64,
    new_image_id: u32,
    width_px: u32,
    height_px: u32,
    rgba: &'a [u8],
    cols: u32,
    old_rows: u32,
    new_rows: u32,
}

fn tail_replace_operations(replace: TailReplace<'_>) -> Result<Vec<TerminalOp>, RenderError> {
    let old_image_id = u32::try_from(replace.old_image_id).map_err(|_| stream_error())?;
    let mut operations = vec![
        TerminalOp::Local(format!("\x1b[{}A\r", replace.old_rows).into_bytes()),
        TerminalOp::Graphics(tmath_core::kitty::kitty_delete_id(old_image_id)),
        TerminalOp::Local(clear_rows(replace.old_rows)),
    ];
    operations.extend(emit_placed_block_cursor(
        replace.new_image_id,
        replace.width_px,
        replace.height_px,
        replace.rgba,
        replace.cols,
        replace.new_rows,
        true,
    ));
    operations.push(TerminalOp::Local(b"\r\n".to_vec()));
    Ok(operations)
}

fn clear_rows(rows: u32) -> Vec<u8> {
    let rows = rows.max(1);
    let mut bytes = Vec::new();
    for row in 0..rows {
        bytes.extend_from_slice(b"\x1b[2K");
        if row + 1 < rows {
            bytes.extend_from_slice(b"\x1b[1B\r");
        }
    }
    if rows > 1 {
        bytes.extend_from_slice(format!("\x1b[{}A\r", rows - 1).as_bytes());
    }
    bytes
}

fn stream_error() -> RenderError {
    RenderError::new(
        SafeErrorRecord {
            code: tmath_render::ErrorCode::RendererFailed,
            retryable: false,
            details: None,
        },
        "stream rendering failed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct_bytes(operations: &[TerminalOp]) -> Vec<u8> {
        let mut bytes = Vec::new();
        tmath_core::placement::write_terminal_ops(&mut bytes, operations, false).unwrap();
        bytes
    }

    #[test]
    fn tail_replace_moves_up_deletes_clears_and_replaces() {
        let operations = tail_replace_operations(TailReplace {
            old_image_id: 7,
            new_image_id: 8,
            width_px: 1,
            height_px: 1,
            rgba: &[0, 0, 0, 0],
            cols: 1,
            old_rows: 3,
            new_rows: 1,
        })
        .unwrap();
        let bytes = direct_bytes(&operations);
        let prefix = concat!(
            "\u{1b}[3A\r",
            "\u{1b}_Ga=d,d=I,i=7,q=2\u{1b}\\",
            "\u{1b}[2K\u{1b}[1B\r\u{1b}[2K\u{1b}[1B\r\u{1b}[2K",
            "\u{1b}[2A\r"
        );
        assert!(bytes.starts_with(prefix.as_bytes()));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("i=8,U=1,c=1,r=1,q=2"));
        assert!(text.ends_with("\x1b[39m\r\n"));
    }

    #[test]
    fn keep_maps_to_zero_terminal_bytes() {
        let plan = Plan {
            ops: vec![PlanOp::Keep { id: 1 }],
            reanchor_from: None,
        };
        let mut emitted = Vec::new();
        for operation in plan.ops {
            if !matches!(operation, PlanOp::Keep { .. }) {
                emitted.push(TerminalOp::Local(b"unexpected".to_vec()));
            }
        }
        assert!(direct_bytes(&emitted).is_empty());
    }
}
