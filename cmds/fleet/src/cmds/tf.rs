use anyhow::Result;
use clap::Parser;
use nix_eval::nix_go_json;
use serde_json::Value;
use tokio::fs::write;
use tracing::info;

use crate::host::Config;

#[derive(Parser)]
pub struct Tf;
impl Tf {
	pub async fn run(&self, config: &Config) -> Result<()> {
		let system = &config.local_system;
		let config = &config.config_field;
		let data: Value = nix_go_json!(config.tf({ system }).config);
		let str = serde_json::to_string_pretty(&data)?;

		write("fleet.tf.json", str.as_bytes()).await?;

		Ok(())
	}
}
