use std::{collections::BTreeMap, process::Command};

use anyhow::Result;
use log::*;

use crate::{command::CommandExt, nix::HOSTS_ATTRIBUTE};

use serde::{Deserialize, Serialize};

use super::db::DbData;

pub fn list_hosts() -> Result<Vec<String>> {
	Ok(Command::new("nix")
		.inherit_stdio()
		.arg("eval")
		.arg(HOSTS_ATTRIBUTE)
		.arg("--apply")
		.arg("builtins.attrNames")
		.run_json()?)
}

#[derive(Serialize, Deserialize, Default)]
pub struct KeyDb {
	host_keys: BTreeMap<String, String>,
}
impl DbData for KeyDb {
	const DB_NAME: &'static str = "keys";
}

impl KeyDb {
	pub fn fetch_key(&mut self, host: &str) -> Result<()> {
		info!("Fetching key for {}", host);
		let key = Command::ssh_on(host, "cat")
			.arg("/etc/ssh/ssh_host_ed25519_key.pub")
			.run_string()?;
		self.host_keys.insert(host.to_owned(), key);
		Ok(())
	}

	pub fn ensure_key_loaded(&mut self, host: &str, force: bool) -> Result<()> {
		if !self.host_keys.contains_key(host) || force {
			self.fetch_key(host)?;
		}
		Ok(())
	}

	pub fn get_host_key(&self, host: &str) -> Result<String> {
		Ok(self
			.host_keys
			.get(host)
			.ok_or_else(|| anyhow::anyhow!("no host key for {}", host))?
			.to_owned())
	}

	pub fn has_key(&self, key: &str) -> bool {
		self.host_keys.contains_key(key)
	}

	pub fn remove_key(&mut self, host: &str) {
		self.host_keys.remove(host);
	}
}
