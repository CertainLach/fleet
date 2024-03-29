use std::{
	env::current_dir,
	ffi::{OsStr, OsString},
	fmt::Display,
	io::Write,
	ops::Deref,
	path::PathBuf,
	str::FromStr,
	sync::{Arc, Mutex, MutexGuard, OnceLock},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{ArgGroup, Parser};
use openssh::SessionBuilder;
use serde::de::DeserializeOwned;
use tempfile::NamedTempFile;

use crate::{
	better_nix_eval::{Field, NixSessionPool},
	command::MyCommand,
	fleetdata::{FleetData, FleetSecret, FleetSharedSecret, SecretData},
	nix_go, nix_go_json,
};

pub struct FleetConfigInternals {
	pub local_system: String,
	pub directory: PathBuf,
	pub opts: FleetOpts,
	pub data: Mutex<FleetData>,
	pub nix_args: Vec<OsString>,
	/// fleet_config.config
	pub config_field: Field,
	/// fleet_config.unchecked.config
	pub config_unchecked_field: Field,

	/// import nixpkgs {system = local};
	pub default_pkgs: Field,
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
	config: Config,
	pub name: String,
	pub local: bool,
	pub session: OnceLock<Arc<openssh::Session>>,

	pub nixos_config: Field,
}
impl ConfigHost {
	async fn open_session(&self) -> Result<Arc<openssh::Session>> {
		assert!(!self.local, "do not open ssh connection to local session");
		// FIXME: TOCTOU
		if let Some(session) = &self.session.get() {
			return Ok((*session).clone());
		};
		let session = SessionBuilder::default();

		let session = session
			.connect(&self.name)
			.await
			.map_err(|e| anyhow!("ssh error while connecting to {}: {e}", self.name))?;
		let session = Arc::new(session);
		self.session.set(session.clone()).expect("TOCTOU happened");
		Ok(session)
	}
	pub async fn mktemp_dir(&self) -> Result<String> {
		let mut cmd = self.cmd("mktemp").await?;
		cmd.arg("-d");
		let path = cmd.run_string().await?;
		Ok(path.trim_end().to_owned())
	}
	pub async fn read_file_bin(&self, path: impl AsRef<OsStr>) -> Result<Vec<u8>> {
		let mut cmd = self.cmd("cat").await?;
		cmd.arg(path);
		cmd.run_bytes().await
	}
	pub async fn read_file_text(&self, path: impl AsRef<OsStr>) -> Result<String> {
		let mut cmd = self.cmd("cat").await?;
		cmd.arg(path);
		cmd.run_string().await
	}
	#[allow(dead_code)]
	pub async fn read_file_json<D: DeserializeOwned>(&self, path: impl AsRef<OsStr>) -> Result<D> {
		let text = self.read_file_text(path).await?;
		Ok(serde_json::from_str(&text)?)
	}
	pub async fn read_file_value<D: FromStr>(&self, path: impl AsRef<OsStr>) -> Result<D>
	where
		<D as FromStr>::Err: Display,
	{
		let text = self.read_file_text(path).await?;
		D::from_str(&text).map_err(|e| anyhow!("failed to parse value: {e}"))
	}
	pub async fn cmd(&self, cmd: impl AsRef<OsStr>) -> Result<MyCommand> {
		if self.local {
			Ok(MyCommand::new(cmd))
		} else {
			let session = self.open_session().await?;
			Ok(MyCommand::new_on(cmd, session))
		}
	}

	pub async fn decrypt(&self, data: SecretData) -> Result<Vec<u8>> {
		let mut cmd = self.cmd("fleet-install-secrets").await?;
		cmd.arg("decrypt").eqarg("--secret", data.encode_z85());
		let encoded = cmd
			.sudo()
			.run_string()
			.await
			.context("failed to call remote host for decrypt")?;
		z85::decode(encoded.trim_end()).context("bad encoded data? outdated host?")
	}
	pub async fn reencrypt(&self, data: SecretData, targets: Vec<String>) -> Result<SecretData> {
		let mut cmd = self.cmd("fleet-install-secrets").await?;
		cmd.arg("reencrypt").eqarg("--secret", data.encode_z85());
		for target in targets {
			let key = self.config.key(&target).await?;
			cmd.eqarg("--targets", key);
		}
		let encoded = cmd
			.sudo()
			.run_string()
			.await
			.context("failed to call remote host for decrypt")?;
		SecretData::decode_z85(encoded.trim_end()).context("bad encoded data? outdated host?")
	}
	/// Returns path for futureproofing, as path might change i.e on conversion to CA
	pub async fn remote_derivation(&self, path: &PathBuf) -> Result<PathBuf> {
		if self.local {
			// Path is located locally, thus already trusted.
			return Ok(path.to_owned());
		}
		let mut nix = MyCommand::new("nix");
		nix.arg("copy")
			.arg("--substitute-on-destination")
			.comparg("--to", format!("ssh-ng://{}", self.name))
			.arg(path);
		nix.run_nix().await.context("nix copy")?;
		Ok(path.to_owned())
	}
	pub async fn systemctl_stop(&self, name: &str) -> Result<()> {
		let mut cmd = self.cmd("systemctl").await?;
		cmd.arg("stop").arg(name);
		cmd.sudo().run().await
	}
	pub async fn systemctl_start(&self, name: &str) -> Result<()> {
		let mut cmd = self.cmd("systemctl").await?;
		cmd.arg("start").arg(name);
		cmd.sudo().run().await
	}

	pub async fn rm_file(&self, path: impl AsRef<OsStr>, sudo: bool) -> Result<()> {
		let mut cmd = self.cmd("rm").await?;
		cmd.arg("-f").arg(path);
		if sudo {
			cmd = cmd.sudo()
		}
		cmd.run().await
	}

	pub async fn list_configured_secrets(&self) -> Result<Vec<String>> {
		let nixos = &self.nixos_config;
		let secrets = nix_go!(nixos.secrets);
		let mut out = Vec::new();
		for name in secrets.list_fields().await? {
			let secret = nix_go!(secrets[{ name }]);
			let is_shared: bool = nix_go_json!(secret.shared);
			if is_shared {
				continue;
			}
			out.push(name);
		}
		Ok(out)
	}
	pub async fn secret_field(&self, name: &str) -> Result<Field> {
		let nixos = &self.nixos_config;
		Ok(nix_go!(nixos.secrets[{ name }]))
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

	pub async fn host(&self, name: &str) -> Result<ConfigHost> {
		let config = &self.config_unchecked_field;
		let nixos_config = nix_go!(config.hosts[{ name }].nixosSystem.config);
		Ok(ConfigHost {
			config: self.clone(),
			name: name.to_owned(),
			local: self.is_local(name),
			session: OnceLock::new(),
			nixos_config,
		})
	}
	pub async fn list_hosts(&self) -> Result<Vec<ConfigHost>> {
		let config = &self.config_unchecked_field;
		let names = nix_go!(config.hosts).list_fields().await?;
		let mut out = vec![];
		for name in names {
			out.push(self.host(&name).await?);
		}
		Ok(out)
	}
	pub async fn system_config(&self, host: &str) -> Result<Field> {
		let fleet_field = &self.config_unchecked_field;
		Ok(nix_go!(fleet_field.hosts[{ host }].nixosSystem.config))
	}

	pub(super) fn data(&self) -> MutexGuard<FleetData> {
		self.data.lock().unwrap()
	}
	pub(super) fn data_mut(&self) -> MutexGuard<FleetData> {
		self.data.lock().unwrap()
	}
	/// Shared secrets configured in fleet.nix or in flake
	pub async fn list_configured_shared(&self) -> Result<Vec<String>> {
		let config_field = &self.config_unchecked_field;
		nix_go!(config_field.sharedSecrets).list_fields().await
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

	pub fn list_secrets(&self, host: &str) -> Vec<String> {
		let data = self.data();
		let Some(secrets) = data.host_secrets.get(host) else {
			return Vec::new();
		};
		secrets.keys().cloned().collect()
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
		let config_field = &self.config_unchecked_field;
		Ok(nix_go_json!(
			config_field.sharedSecrets[{ secret }].expectedOwners
		))
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

		let builtins_field = Field::field(root_field.clone(), "builtins").await?;
		if self.local_system == "detect" {
			self.local_system = nix_go_json!(builtins_field.currentSystem);
		}
		let local_system = self.local_system.clone();

		let fleet_root = Field::field(root_field, "fleetConfigurations").await?;
		let fleet_field = nix_go!(fleet_root.default);

		let config_field = nix_go!(fleet_field.config);
		let config_unchecked_field = nix_go!(fleet_field.unchecked.config);

		let import = nix_go!(builtins_field.import);
		let overlays = nix_go!(fleet_field.overlays);
		let nixpkgs = nix_go!(fleet_field.nixpkgs | import);

		let default_pkgs = nix_go!(nixpkgs(Obj {
			overlays,
			system: { self.local_system.clone() },
		}));

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
			config_field,
			config_unchecked_field,
			default_pkgs,
		})))
	}
}
