use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    native_engine_spike::run_main_spike()
}
