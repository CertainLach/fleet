use std::{
	env::current_dir,
	ffi::OsString,
	io::Write,
	ops::Deref,
	path::PathBuf,
	sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{ArgGroup, Parser};
use openssh::SessionBuilder;
use tempfile::NamedTempFile;

use crate::{
	better_nix_eval::{Field, NixSessionPool},
	command::MyCommand,
	fleetdata::{FleetData, FleetSecret, FleetSharedSecret},
	nix_path,
};

pub struct FleetConfigInternals {
	pub local_system: String,
	pub directory: PathBuf,
	pub opts: FleetOpts,
	pub data: Mutex<FleetData>,
	pub nix_args: Vec<OsString>,
	/// fleetConfigurations.<name>.<localSystem>
	pub fleet_field: Field,
	/// fleet_config.configUnchecked
	pub config_field: Field,
}

#[derive(Clone)]
pub struct Config(Arc<FleetConfigInternals>);

impl Deref for Config {
	type Target = FleetConfigInternals;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

pub struct ConfigHost {
	pub name: String,
}
impl ConfigHost {
	async fn open_session(&self) -> Result<openssh::Session> {
		let mut session = SessionBuilder::default();

		session
			.connect(&self.name)
			.await
			.map_err(|e| anyhow!("ssh error: {e}"))
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

	pub async fn run_on(&self, host: &str, mut command: MyCommand, sudo: bool) -> Result<()> {
		if sudo {
			command = command.sudo();
		}
		if !self.is_local(host) {
			command = command.ssh(host);
		}
		command.run().await
	}
	pub async fn run_string_on(
		&self,
		host: &str,
		mut command: MyCommand,
		sudo: bool,
	) -> Result<String> {
		if sudo {
			command = command.sudo();
		}
		if !self.is_local(host) {
			command = command.ssh(host);
		}
		command.run_string().await
	}

	pub async fn list_hosts(&self) -> Result<Vec<ConfigHost>> {
		let names = self
			.fleet_field
			.select(nix_path!(.configuredHosts))
			.await?
			.list_fields()
			.await?;
		let mut out = vec![];
		for name in names {
			out.push(ConfigHost { name })
		}
		Ok(out)
	}
	pub async fn system_config(&self, host: &str) -> Result<Field> {
		self.fleet_field
			.select(nix_path!(.configuredSystems.{host}.config))
			.await
	}

	pub(super) fn data(&self) -> MutexGuard<FleetData> {
		self.data.lock().unwrap()
	}
	pub(super) fn data_mut(&self) -> MutexGuard<FleetData> {
		self.data.lock().unwrap()
	}
	/// Shared secrets configured in fleet.nix or in flake
	pub async fn list_configured_shared(&self) -> Result<Vec<String>> {
		self.config_field
			.select(nix_path!(.sharedSecrets))
			.await?
			.list_fields()
			.await
	}
	/// Shared secrets configured in fleet.nix
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

	pub async fn decrypt_on_host(&self, host: &str, data: Vec<u8>) -> Result<Vec<u8>> {
		let data = z85::encode(&data);
		let mut cmd = MyCommand::new("fleet-install-secrets");
		cmd.arg("decrypt").eqarg("--secret", data);
		cmd = cmd.sudo().ssh(host);
		let encoded = cmd
			.run_string()
			.await
			.context("failed to call remote host for decrypt")?
			.trim()
			.to_owned();
		z85::decode(encoded).context("bad encoded data? outdated host?")
	}
	pub async fn reencrypt_on_host(
		&self,
		host: &str,
		data: Vec<u8>,
		targets: Vec<String>,
	) -> Result<Vec<u8>> {
		let data = z85::encode(&data);
		let mut recmd = MyCommand::new("fleet-install-secrets");
		recmd.arg("reencrypt").eqarg("--secret", data);
		for target in targets {
			recmd.eqarg("--targets", target);
		}
		recmd = recmd.sudo().ssh(host);
		let encoded = recmd
			.run_string()
			.await
			.context("failed to call remote host for decrypt")?
			.trim()
			.to_owned();
		z85::decode(encoded).context("bad encoded data? outdated host?")
	}

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
	pub fn shared_secret(&self, secret: &str) -> Result<FleetSharedSecret> {
		let data = self.data();
		let Some(secret) = data.shared_secrets.get(secret) else {
			bail!("no shared secret {secret}");
		};
		Ok(secret.clone())
	}
	pub async fn shared_secret_expected_owners(&self, secret: &str) -> Result<Vec<String>> {
		self.config_field
			.select(nix_path!(.sharedSecrets.{secret}.expectedOwners))
			.await?
			.as_json()
			.await
	}

	pub fn save(&self) -> Result<()> {
		let mut tempfile = NamedTempFile::new_in(self.directory.clone())?;
		let data = nixlike::serialize(&self.data() as &FleetData)?;
		tempfile.write_all(
			format!(
				"# This file contains fleet state and shouldn't be edited by hand\n\n{}\n\n# vim: ts=2 et nowrap\n",
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

	/// Override detected system for host, to perform builds via
	/// binfmt-declared qemu instead of trying to crosscompile
	#[clap(long, default_value = "detect")]
	pub local_system: String,
}

impl FleetOpts {
	pub async fn build(mut self, nix_args: Vec<OsString>) -> Result<Config> {
		if self.localhost.is_none() {
			self.localhost
				.replace(hostname::get().unwrap().to_str().unwrap().to_owned());
		}
		let directory = current_dir()?;

		let pool = NixSessionPool::new(directory.as_os_str().to_owned(), nix_args.clone()).await?;
		let root_field = pool.get().await?;

		if self.local_system == "detect" {
			let builtins_field = Field::field(root_field.clone(), "builtins").await?;
			let system = builtins_field
				.select(nix_path!(.currentSystem))
				.await?;
			self.local_system = system.as_json().await?;
		}
		let local_system = self.local_system.clone();

		let fleet_root = Field::field(root_field, "fleetConfigurations").await?;

		let fleet_field = fleet_root
			.select(nix_path!(.default))
			.await?;
		let config_field = fleet_field
			.select(nix_path!(.configUnchecked))
			.await?;

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
			fleet_field,
			config_field,
		})))
	}
}
