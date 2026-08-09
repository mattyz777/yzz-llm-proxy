use anyhow::Result;

use crate::{config::Config, tracer::init_tracing};

mod config;
mod constant;
mod provider;
mod state;
mod tracer;


#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = Config::init()?;

    println!("Hello, world!");
    Ok(())
}
