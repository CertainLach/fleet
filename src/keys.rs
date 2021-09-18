use std::str::FromStr;

use crate::{command::CommandExt, host::Config};
use anyhow::{anyhow, Result};
use log::warn;

impl Config {
	pub fn cached_key(&self, host: &str) -> Option<String> {
		let data = self.data();
		let key = data.hosts.get(host).map(|h| &h.encryption_key);
		if let Some(key) = key {
			if key.is_empty() {
				return None;
			}
		}
		key.cloned()
	}
	pub fn update_key(&self, host: &str, key: String) {
		let mut data = self.data_mut();
		let host = data.hosts.entry(host.to_string()).or_default();
		host.encryption_key = key.trim().to_string();
	}
	pub fn update_secret(&self, host: &str, name: &str, value: &[u8]) {
		let mut data = self.data_mut();
		let host = data.hosts.entry(host.to_string()).or_default();
		host.encrypted_secrets.insert(
			name.to_string(),
			format!("[ENCRYPTED:{}]", base64::encode(value)),
		);
	}

	pub fn key(&self, host: &str) -> anyhow::Result<String> {
		if let Some(key) = self.cached_key(host) {
			Ok(key)
		} else {
			warn!("Loading key for {}", host);
			let key = self
				.command_on("host", "cat", false)
				.arg("/etc/ssh/ssh_host_ed25519_key.pub")
				.run_string()?;
			self.update_key(host, key.clone());
			Ok(key)
		}
	}
	pub fn recipient(&self, host: &str) -> anyhow::Result<age::ssh::Recipient> {
		let key = self.key(host)?;
		age::ssh::Recipient::from_str(&key).map_err(|e| anyhow!("parse recipient error: {:?}", e))
	}

	pub fn orphaned_data(&self) -> Result<Vec<String>> {
		let mut out = Vec::new();
		let host_names = self.list_hosts()?;
		for hostname in self
			.data()
			.hosts
			.iter()
			.filter(|(_, host)| !host.encryption_key.is_empty())
			.map(|(n, _)| n)
		{
			if !host_names.contains(&hostname.to_owned()) {
				out.push(hostname.to_owned())
			}
		}

		Ok(out)
	}
}
