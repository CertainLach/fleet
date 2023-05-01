use std::{
	cell::{Ref, RefCell, RefMut},
	env::current_dir,
	ffi::{OsStr, OsString},
	io::Write,
	ops::Deref,
	path::PathBuf,
	sync::Arc,
};

use anyhow::{Result, bail, Context};
use clap::{ArgGroup, Parser};
use serde::de::DeserializeOwned;
use tempfile::NamedTempFile;
use tokio::process::Command;

use crate::{
	command::CommandExt,
	fleetdata::{FleetData, FleetSecret, FleetSharedSecret},
};

pub struct FleetConfigInternals {
	pub local_system: String,
	pub directory: PathBuf,
	pub opts: FleetOpts,
	pub data: RefCell<FleetData>,
	pub nix_args: Vec<OsString>,
}

#[derive(Clone)]
pub struct Config(Arc<FleetConfigInternals>);

impl Deref for Config {
	type Target = FleetConfigInternals;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl Config {
	pub fn should_skip(&self, host: &str) -> bool {
		if !self.opts.skip.is_empty() {
			self.opts.skip.iter().any(|h| h as &str == host)
		} else if !self.opts.only.is_empty() {
			!self.opts.only.iter().any(|h| h as &str == host)
		} else {
			false
		}
	}
	pub fn is_local(&self, host: &str) -> bool {
		self.opts.localhost.as_ref().map(|s| s as &str) == Some(host)
	}

	pub fn command_on(&self, host: &str, program: impl AsRef<OsStr>, sudo: bool) -> Command {
		if self.is_local(host) {
			if sudo {
				let mut cmd = Command::new("sudo");
				cmd.arg(program);
				cmd
			} else {
				Command::new(program)
			}
		} else {
			let mut cmd = Command::new("ssh");
			cmd.arg(host).arg("--");
			if sudo {
				cmd.arg("sudo");
			}
			cmd.arg(program);
			cmd
		}
	}

	pub fn configuration_attr_name(&self, name: &str) -> OsString {
		let mut str = self.directory.as_os_str().to_owned();
		str.push("#");
		str.push(&format!(
			"fleetConfigurations.default.{}.{}",
			self.local_system, name
		));
		str
	}

	pub async fn list_hosts(&self) -> Result<Vec<String>> {
		Command::new("nix")
			.arg("eval")
			.arg(self.configuration_attr_name("configuredHosts"))
			.args(["--apply", "builtins.attrNames", "--json", "--show-trace"])
			.args(&self.nix_args)
			.run_nix_json()
			.await
	}
	pub async fn shared_config_attr<T: DeserializeOwned>(&self, attr: &str) -> Result<T> {
		Command::new("nix")
			.arg("eval")
			.arg(self.configuration_attr_name(&format!("configUnchecked.{}", attr)))
			.args(["--json", "--show-trace"])
			.args(&self.nix_args)
			.run_nix_json()
			.await
	}
	pub async fn shared_config_attr_names(&self, attr: &str) -> Result<Vec<String>> {
		Command::new("nix")
			.arg("eval")
			.arg(self.configuration_attr_name(&format!("configUnchecked.{}", attr)))
			.args(["--apply", "builtins.attrNames"])
			.args(["--json", "--show-trace"])
			.args(&self.nix_args)
			.run_nix_json()
			.await
	}
	pub async fn config_attr<T: DeserializeOwned>(&self, host: &str, attr: &str) -> Result<T> {
		Command::new("nix")
			.arg("eval")
			.arg(
				self.configuration_attr_name(&format!(
					"configuredSystems.{}.config.{}",
					host, attr
				)),
			)
			.args(["--json", "--show-trace"])
			.args(&self.nix_args)
			.run_nix_json()
			.await
	}

	pub(super) fn data(&self) -> Ref<FleetData> {
		self.data.borrow()
	}
	pub(super) fn data_mut(&self) -> RefMut<FleetData> {
		self.data.borrow_mut()
	}

	pub fn list_shared(&self) -> Vec<String> {
		let data = self.data();
		data.shared_secrets.keys().cloned().collect()
	}
	pub fn has_shared(&self, name: &str) -> bool {
		let data = self.data();
		data.shared_secrets.contains_key(name)
	}
	pub fn replace_shared(&self, name: String, shared: FleetSharedSecret) {
		let mut data = self.data_mut();
		data.shared_secrets.insert(name.to_owned(), shared);
	}
	pub fn remove_shared(&self, secret: &str) {
		let mut data = self.data_mut();
		data.shared_secrets.remove(secret);
	}

	pub fn list_secrets(&self, host: &str) -> Vec<String> {
		let data = self.data();
		let Some(host_secrets) = data.host_secrets.get(host) else {
			return Vec::new(); 
		};
		host_secrets.keys().cloned().collect()
	}
	pub fn has_secret(&self, host: &str, secret: &str) -> bool {
		let data = self.data();
		let Some(host_secrets) = data.host_secrets.get(host) else {
			return false; 
		};
		host_secrets.contains_key(secret)
	}
	pub fn insert_secret(&self, host: &str, secret: String, value: FleetSecret) {
		let mut data = self.data_mut();
		let host_secrets = data.host_secrets.entry(host.to_owned()).or_default();
		host_secrets.insert(secret, value);
	}

	pub async fn decrypt_on_host(&self, host: &str, data: Vec<u8>) -> Result<Vec<u8>>{
		let data = z85::encode(&data);
		let encoded = self.command_on(host, "fleet-install-secrets", true)
			.arg("decrypt")
			.arg("--secret")
			.arg(data).run_string().await.context("failed to call remote host for decrypt")?.trim().to_owned();
		Ok(z85::decode(encoded).context("bad encoded data? outdated host?")?)
	}
	pub async fn reencrypt_on_host(&self, host: &str, data: Vec<u8>, targets: Vec<String>) -> Result<Vec<u8>>{
		let data = z85::encode(&data);
		let mut recmd = self.command_on(host, "fleet-install-secrets", true);
		recmd
			.arg("reencrypt")
			.arg("--secret")
			.arg(format!("\"{}\"", data.replace('$', "\\$")));
		for target in targets {
			recmd.arg("--targets");
			recmd.arg(format!("\"{target}\""));
		}
		let encoded = recmd.run_string().await.context("failed to call remote host for decrypt")?.trim().to_owned();
		Ok(z85::decode(encoded).context("bad encoded data? outdated host?")?)
	}

	#[must_use]
	pub fn host_secret(&self, host: &str, secret: &str) -> Result<FleetSecret> {
		let data = self.data();
		let Some(host_secrets) = data.host_secrets.get(host) else {
            bail!("no secrets for machine {host}");
        };
		let Some(secret) = host_secrets.get(secret) else {
            bail!("machine {host} has no secret {secret}");
        };
		Ok(secret.clone())
	}
	#[must_use]
	pub fn shared_secret(&self, secret: &str) -> Result<FleetSharedSecret> {
		let data = self.data();
		let Some(secret) = data.shared_secrets.get(secret) else {
			bail!("no shared secret {secret}");
		};
		Ok(secret.clone())
	}

	pub fn save(&self) -> Result<()> {
		let mut tempfile = NamedTempFile::new_in(self.directory.clone())?;
		let data = nixlike::serialize(&self.data() as &FleetData)?;
		tempfile.write_all(
			format!(
				"# This file contains fleet state and shouldn't be edited by hand\n\n{}\n",
				data
			)
			.as_bytes(),
		)?;
		let mut fleet_data_path = self.directory.clone();
		fleet_data_path.push("fleet.nix");
		tempfile.persist(fleet_data_path)?;
		Ok(())
	}
}

#[derive(Parser, Clone)]
#[clap(group = ArgGroup::new("target_hosts"))]
pub struct FleetOpts {
	/// All hosts except those would be skipped
	#[clap(long, number_of_values = 1, group = "target_hosts")]
	only: Vec<String>,

	/// Hosts to skip
	#[clap(long, number_of_values = 1, group = "target_hosts")]
	skip: Vec<String>,

	/// Host, which should be threaten as current machine
	#[clap(long)]
	pub localhost: Option<String>,

	// TODO: unhardcode x86_64-linux
	/// Override detected system for host, to perform builds via
	/// binfmt-declared qemu instead of trying to crosscompile
	#[clap(long, default_value = "x86_64-linux")]
	pub local_system: String,
}

impl FleetOpts {
	pub async fn build(mut self, nix_args: Vec<OsString>) -> Result<Config> {
		let local_system = self.local_system.clone();
		if self.localhost.is_none() {
			self.localhost
				.replace(hostname::get().unwrap().to_str().unwrap().to_owned());
		}
		let directory = current_dir()?;

		let mut fleet_data_path = directory.clone();
		fleet_data_path.push("fleet.nix");
		let bytes = std::fs::read_to_string(fleet_data_path)?;
		let data = nixlike::parse_str(&bytes)?;

		Ok(Config(Arc::new(FleetConfigInternals {
			opts: self,
			directory,
			data,
			local_system,
			nix_args,
		})))
	}
}
