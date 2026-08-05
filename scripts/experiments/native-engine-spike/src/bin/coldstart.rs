use std::error::Error;
use std::time::Instant;

fn main() -> Result<(), Box<dyn Error>> {
    let process_start = Instant::now();
    let metrics = native_engine_spike::render_cold_start_block(process_start)?;
    println!(
        "{{\"engine_build_ms\":{:.3}, \"first_render_ms\":{:.3}, \"total_ms\":{:.3}, \"png_bytes\":{}, \"fonts_loaded\":{}}}",
        metrics.engine_build.as_secs_f64() * 1_000.0,
        metrics.first_render.as_secs_f64() * 1_000.0,
        process_start.elapsed().as_secs_f64() * 1_000.0,
        metrics.png_bytes,
        metrics.fonts_loaded,
    );
    Ok(())
}
