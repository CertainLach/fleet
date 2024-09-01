use std::{
	collections::{BTreeMap, HashMap},
	ffi::OsString,
	path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use fleet_base::host::Config;
use nix_eval::nix_go;
use serde::Deserialize;
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio::{
	fs::{self, create_dir_all},
	process::Command,
};
use tracing::debug;

#[derive(Deserialize, Debug)]
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
pub struct Tf {
	args: Vec<OsString>,
}
impl Tf {
	pub async fn run(&self, config: &Config) -> Result<()> {
		let dir = config.directory.join(".fleet/tf/default");
		// TODO: consider postponing fleet init until this step, as it might be
		// highly preferred to extract terraform configuration using multithreaded nix or
		// lazy-trees nix. lazy-trees nix is very fast and perfect for this task.
		{
			debug!("generating terraform configs");
			let system = &config.local_system;
			let config = &config.config_field;
			let data: HashMap<String, PathBuf> = nix_go!(config.tf({ system })).build().await?;
			let data = &data["out"];
			let data = fs::read(&data).await?;

			create_dir_all(&dir).await?;

			let tmp = NamedTempFile::new_in(&dir)?;
			fs::write(tmp.path(), data).await?;
			tmp.persist(dir.join("fleet.tf.json"))?;
		}

		{
			debug!("running terraform command");
			Command::new("terraform")
				.current_dir(&dir)
				.args(&self.args)
				.status()
				.await?;
		}
		{
			debug!("syncing terraform data");
			let data = Command::new("terraform")
				.current_dir(dir)
				.arg("output")
				.arg("-json")
				.arg("fleet")
				.output()
				.await?;
			let tf_data: TfData = serde_json::from_slice(&data.stdout)
				.context("failed to parse terraform fleet output")?;

			let mut data = config.data();
			debug!("synchronized done = {tf_data:?}");
			data.extra.insert(
				"terraformHosts".to_owned(),
				serde_json::to_value(tf_data.hosts).expect("should be valid extra"),
			);
		}

		Ok(())
	}
}
