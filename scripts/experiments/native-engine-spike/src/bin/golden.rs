use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let run = native_engine_spike::golden::run_golden()?;
    println!("{}", run.summary_line());
    Ok(())
}
