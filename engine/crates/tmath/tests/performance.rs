//! AT-3-901..905 reference-machine performance suite.
//!
//! Always-run tests verify the suite is registered. Release-only gates:
//!
//! ```sh
//! cargo test -p tmath --release performance -- --ignored
//! ```

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tmath_render::{parse_blocks_limited, render_block, Block, Limits, RenderOptions};

const WARMUP_SAMPLES: usize = 3;
const SAMPLE_COUNT: usize = 20;
const G1_P50_MS: f64 = 10.0;
const G1_P95_MS: f64 = 30.0;
const G4_P50_MS: f64 = 300.0;

fn percentile(values: &mut [Duration], quantile: f64) -> Duration {
    assert!(!values.is_empty());
    values.sort();
    let position = quantile.clamp(0.0, 1.0) * (values.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        values[lower]
    } else {
        let weight = position - lower as f64;
        let lower_ms = values[lower].as_secs_f64();
        let upper_ms = values[upper].as_secs_f64();
        Duration::from_secs_f64(lower_ms * (1.0 - weight) + upper_ms * weight)
    }
}

fn warm_corpus_blocks() -> Vec<Block> {
    let document = concat!(
        "# Warm corpus\n\n",
        "Inline math $E=mc^2$ in prose.\n\n",
        "$$\\int_0^1 x^2 \\, dx = \\frac{1}{3}$$\n\n",
        "- one\n- two\n- three\n\n",
        "| A | B |\n| - | - |\n| 1 | 2 |\n"
    );
    parse_blocks_limited(document, &Limits::default()).unwrap()
}

#[test]
fn performance_suite_is_registered() {
    const _: () = assert!(SAMPLE_COUNT >= WARMUP_SAMPLES);
    assert!(!warm_corpus_blocks().is_empty());
}

#[test]
#[ignore = "run with `cargo test -p tmath --release warm_block_render_meets_g1 -- --ignored`"]
fn warm_block_render_meets_g1() {
    let options = RenderOptions::default();
    let blocks = warm_corpus_blocks();
    for block in &blocks {
        let _ = render_block(block, &options).unwrap();
    }

    let mut latencies = Vec::with_capacity(SAMPLE_COUNT * blocks.len());
    for sample in 0..SAMPLE_COUNT {
        for block in &blocks {
            if sample < WARMUP_SAMPLES {
                let _ = render_block(block, &options).unwrap();
                continue;
            }
            let started = Instant::now();
            let rendered = render_block(block, &options).unwrap();
            assert!(!rendered.png.is_empty());
            latencies.push(started.elapsed());
        }
    }

    let p50 = percentile(&mut latencies.clone(), 0.50);
    let p95 = percentile(&mut latencies, 0.95);
    eprintln!(
        "G1 warm block render: p50={:.3} ms p95={:.3} ms (budget p50={G1_P50_MS} p95={G1_P95_MS})",
        p50.as_secs_f64() * 1_000.0,
        p95.as_secs_f64() * 1_000.0
    );
    assert!(
        p50.as_secs_f64() * 1_000.0 <= G1_P50_MS,
        "G1 p50 exceeded budget: {p50:?}"
    );
    assert!(
        p95.as_secs_f64() * 1_000.0 <= G1_P95_MS,
        "G1 p95 exceeded budget: {p95:?}"
    );
}

#[test]
#[ignore = "run with `cargo test -p tmath --release cold_start_render_meets_g4 -- --ignored`"]
fn cold_start_render_meets_g4() {
    let document = "# Cold start\n\nProse with $x=1$.\n\n";
    let mut latencies = Vec::with_capacity(SAMPLE_COUNT);

    for run in 0..SAMPLE_COUNT {
        let started = Instant::now();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tmath"));
        command
            .args(["render", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("tmath binary must spawn");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(document.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "cold render failed on run {run}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        latencies.push(started.elapsed());
    }

    let p50 = percentile(&mut latencies.clone(), 0.50);
    let p95 = percentile(&mut latencies, 0.95);
    eprintln!(
        "G4 cold start render: p50={:.3} ms p95={:.3} ms (budget p50={G4_P50_MS})",
        p50.as_secs_f64() * 1_000.0,
        p95.as_secs_f64() * 1_000.0
    );
    assert!(
        p50.as_secs_f64() * 1_000.0 <= G4_P50_MS,
        "G4 p50 exceeded budget: {p50:?}"
    );
    let _ = p95;
}
