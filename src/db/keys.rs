use std::collections::BTreeMap;

use anyhow::Result;
use log::*;

use crate::{
	command::ssh_command,
	nix::{NixEval, HOSTS_ATTRIBUTE},
};

use serde::{Deserialize, Serialize};

use super::db::DbData;

pub fn list_hosts() -> Result<Vec<String>> {
	Ok(NixEval::new(HOSTS_ATTRIBUTE.into())
		.apply("builtins.attrNames".into())
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
		let key = ssh_command(host, &["cat", "/etc/ssh/ssh_host_ed25519_key.pub"])?
			.as_str()?
			.trim()
			.to_owned();
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
