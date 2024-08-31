use std::{
	collections::{BTreeMap, HashMap},
	path::PathBuf,
};

use anyhow::{bail, Context, Result};
use clap::Parser;
use fleet_base::host::Config;
use nix_eval::nix_go;
use serde::Deserialize;
use serde_json::Value;
use tokio::{fs::copy, process::Command};

#[derive(Deserialize)]
pub struct TfData {
	// Dummy
	#[allow(dead_code)]
	managed: bool,
	// Host => Data
	#[serde(default)]
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	pub hosts: BTreeMap<String, Value>,
}

#[derive(Parser)]
pub enum Tf {
	/// Generate fleet.tf.json file for running terraform.
	Generate,
	/// Fetch data from terraform to fleet.
	Refresh,
}
impl Tf {
	pub async fn run(&self, config: &Config) -> Result<()> {
		match self {
			Tf::Generate => {
				let system = &config.local_system;
				let config = &config.config_field;
				let data: HashMap<String, PathBuf> = nix_go!(config.tf({ system })).build().await?;
				let data = &data["out"];

				copy(data, "fleet.tf.json").await?;
			}
			Tf::Refresh => {
				let cmd = Command::new("terraform").arg("refresh").status().await?;
				if !cmd.success() {
					bail!("terraform refresh failed")
				}

				let data = Command::new("terraform")
					.arg("output")
					.arg("-json")
					.arg("fleet")
					.output()
					.await?;
				let tf_data: TfData = serde_json::from_slice(&data.stdout)
					.context("failed to parse terraform fleet output")?;

				let mut data = config.data();
				data.extra.insert(
					"terraformHosts".to_owned(),
					serde_json::to_value(tf_data.hosts).expect("should be valid extra"),
				);
			}
		}

		Ok(())
	}
}
