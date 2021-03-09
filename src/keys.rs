use crate::{
	command::CommandExt,
	host::{FleetConfig, Host},
};
use anyhow::Result;
use log::warn;
use std::{
	fs::{create_dir_all, metadata, read, read_dir, write},
	path::PathBuf,
};

impl FleetConfig {
	fn host_keys_dir(&self) -> Result<PathBuf> {
		let mut out = self.data_dir().clone();
		out.push("host_keys");
		create_dir_all(&out)?;
		Ok(out)
	}

	fn host_key_file(&self, host: &str) -> Result<PathBuf> {
		let mut dir = self.host_keys_dir()?;
		dir.push(format!("{}.asc", host));
		Ok(dir)
	}

	pub fn list_orphaned_keys(&self) -> Result<Vec<(String, PathBuf)>> {
		let mut out = Vec::new();
		let host_names = self.list_host_names()?;
		for file in read_dir(&self.host_keys_dir()?)? {
			let file = file?;
			anyhow::ensure!(
				file.file_type()?.is_file(),
				"host_keys dir should contain only files"
			);
			let name = file.file_name();
			let name = name.to_str().unwrap();
			if let Some(hostname) = name.strip_suffix(".asc") {
				if !host_names.contains(&hostname.to_owned()) {
					out.push((hostname.to_owned(), file.path()))
				}
			} else {
				out.push(("<unknown>".to_owned(), file.path()))
			}
		}

		Ok(out)
	}
}

impl Host {
	pub fn key(&self) -> anyhow::Result<String> {
		let key_path = self.fleet_config.host_key_file(&self.hostname)?;
		if metadata(&key_path).map(|m| m.is_file()).unwrap_or(false) {
			Ok(String::from_utf8(read(key_path)?)?)
		} else {
			warn!("Loading key for {}", self.hostname);
			let key = self
				.command_on("cat", false)
				.arg("/etc/ssh/ssh_host_ed25519_key.pub")
				.run_string()?;
			write(key_path, key.clone())?;
			Ok(key)
		}
	}
}
