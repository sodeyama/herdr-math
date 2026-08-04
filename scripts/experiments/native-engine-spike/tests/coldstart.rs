use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const SAMPLE_COUNT: usize = 10;
const COLD_START_LIMIT_MS: f64 = 300.0;

#[derive(Clone, Copy, Debug)]
struct BinaryMetrics {
    engine_build_ms: f64,
    first_render_ms: f64,
    total_ms: f64,
    png_bytes: usize,
    fonts_loaded: usize,
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    run: usize,
    wall_clock_ms: f64,
    binary: BinaryMetrics,
}

#[test]
fn cold_start_p50_under_300ms() -> Result<(), Box<dyn Error>> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let build = Command::new("cargo")
        .current_dir(manifest_dir)
        .args(["build", "--release", "--offline", "--bin", "coldstart"])
        .status()?;
    assert!(build.success(), "The release coldstart binary must build");

    let binary = release_binary(manifest_dir);
    let warmup = run_coldstart(&binary, 0)?;
    eprintln!(
        "warmup wall={:.3} ms total={:.3} ms",
        warmup.wall_clock_ms, warmup.binary.total_ms
    );

    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for run in 1..=SAMPLE_COUNT {
        let sample = run_coldstart(&binary, run)?;
        eprintln!(
            "run {:02}: wall={:.3} ms engine_build={:.3} ms first_render={:.3} ms total={:.3} ms png_bytes={} fonts_loaded={}",
            run,
            sample.wall_clock_ms,
            sample.binary.engine_build_ms,
            sample.binary.first_render_ms,
            sample.binary.total_ms,
            sample.binary.png_bytes,
            sample.binary.fonts_loaded,
        );
        samples.push(sample);
    }

    let wall_values = samples
        .iter()
        .map(|sample| sample.wall_clock_ms)
        .collect::<Vec<_>>();
    let total_values = samples
        .iter()
        .map(|sample| sample.binary.total_ms)
        .collect::<Vec<_>>();
    let wall_p50 = percentile(&wall_values, 0.50);
    let wall_p95 = percentile(&wall_values, 0.95);
    let total_p50 = percentile(&total_values, 0.50);
    let total_p95 = percentile(&total_values, 0.95);
    eprintln!(
        "wall-clock p50={wall_p50:.3} ms p95={wall_p95:.3} ms; in-process total p50={total_p50:.3} ms p95={total_p95:.3} ms"
    );

    let out_dir = manifest_dir.join("out");
    fs::create_dir_all(&out_dir)?;
    fs::write(
        out_dir.join("coldstart-summary.json"),
        summary_json(warmup, &samples, wall_p50, wall_p95, total_p50, total_p95),
    )?;

    assert!(
        wall_p50 <= COLD_START_LIMIT_MS,
        "wall-clock p50 was {wall_p50:.3} ms, exceeding the {COLD_START_LIMIT_MS:.0} ms limit"
    );
    Ok(())
}

fn release_binary(manifest_dir: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "coldstart.exe"
    } else {
        "coldstart"
    };
    manifest_dir.join("target").join("release").join(name)
}

fn run_coldstart(binary: &Path, run: usize) -> Result<Sample, Box<dyn Error>> {
    let wall_started = Instant::now();
    let output = Command::new(binary).output()?;
    let wall_clock_ms = wall_started.elapsed().as_secs_f64() * 1_000.0;
    assert!(
        output.status.success(),
        "coldstart exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout)?;
    let line = stdout.trim_end_matches(['\r', '\n']);
    assert!(
        !line.contains('\n') && !line.contains('\r'),
        "coldstart must print exactly one JSON line"
    );
    Ok(Sample {
        run,
        wall_clock_ms,
        binary: parse_metrics(line)?,
    })
}

fn parse_metrics(line: &str) -> Result<BinaryMetrics, Box<dyn Error>> {
    let body = line
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or("coldstart output is not a JSON object")?;
    let mut engine_build_ms = None;
    let mut first_render_ms = None;
    let mut total_ms = None;
    let mut png_bytes = None;
    let mut fonts_loaded = None;
    for field in body.split(',') {
        let (key, value) = field
            .split_once(':')
            .ok_or("coldstart JSON field has no colon")?;
        match key.trim().trim_matches('"') {
            "engine_build_ms" => engine_build_ms = Some(value.trim().parse()?),
            "first_render_ms" => first_render_ms = Some(value.trim().parse()?),
            "total_ms" => total_ms = Some(value.trim().parse()?),
            "png_bytes" => png_bytes = Some(value.trim().parse()?),
            "fonts_loaded" => fonts_loaded = Some(value.trim().parse()?),
            other => return Err(format!("unexpected coldstart JSON field: {other}").into()),
        }
    }
    let metrics = BinaryMetrics {
        engine_build_ms: engine_build_ms.ok_or("missing engine_build_ms")?,
        first_render_ms: first_render_ms.ok_or("missing first_render_ms")?,
        total_ms: total_ms.ok_or("missing total_ms")?,
        png_bytes: png_bytes.ok_or("missing png_bytes")?,
        fonts_loaded: fonts_loaded.ok_or("missing fonts_loaded")?,
    };
    assert!(metrics.png_bytes > 0, "coldstart must produce PNG bytes");
    assert!(metrics.fonts_loaded > 0, "coldstart must load font faces");
    Ok(metrics)
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let weight = position - lower as f64;
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

fn summary_json(
    warmup: Sample,
    samples: &[Sample],
    wall_p50: f64,
    wall_p95: f64,
    total_p50: f64,
    total_p95: f64,
) -> String {
    let mut json = String::from("{\n");
    json.push_str(&format!("  \"warmup\": {},\n", sample_json(warmup)));
    json.push_str("  \"samples\": [\n");
    for (index, sample) in samples.iter().enumerate() {
        json.push_str(&format!(
            "    {}{}\n",
            sample_json(*sample),
            if index + 1 == samples.len() { "" } else { "," }
        ));
    }
    json.push_str("  ],\n");
    json.push_str(&format!(
        "  \"percentiles_ms\": {{\n    \"wall_clock\": {{ \"p50\": {wall_p50:.3}, \"p95\": {wall_p95:.3} }},\n    \"total\": {{ \"p50\": {total_p50:.3}, \"p95\": {total_p95:.3} }}\n  }}\n"
    ));
    json.push_str("}\n");
    json
}

fn sample_json(sample: Sample) -> String {
    format!(
        "{{ \"run\": {}, \"wall_clock_ms\": {:.3}, \"engine_build_ms\": {:.3}, \"first_render_ms\": {:.3}, \"total_ms\": {:.3}, \"png_bytes\": {}, \"fonts_loaded\": {} }}",
        sample.run,
        sample.wall_clock_ms,
        sample.binary.engine_build_ms,
        sample.binary.first_render_ms,
        sample.binary.total_ms,
        sample.binary.png_bytes,
        sample.binary.fonts_loaded,
    )
}
