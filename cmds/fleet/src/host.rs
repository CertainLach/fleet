use std::{
	cell::{LazyCell, OnceCell},
	collections::BTreeMap,
	env::current_dir,
	ffi::{OsStr, OsString},
	fmt::Display,
	io::Write,
	ops::Deref,
	path::PathBuf,
	str::FromStr,
	sync::{Arc, Mutex, MutexGuard, OnceLock},
};

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::Parser;
use fleet_shared::SecretData;
use nix_eval::{nix_go, nix_go_json, util::assert_warn, NixSessionPool, Value};
use nom::{
	bytes::complete::take_while1,
	character::complete::char,
	combinator::{map, opt},
	multi::separated_list1,
	sequence::{preceded, separated_pair},
};
use openssh::SessionBuilder;
use serde::de::DeserializeOwned;
use tempfile::NamedTempFile;
use tracing::error;

use crate::{
	command::MyCommand,
	fleetdata::{FleetData, FleetSecret, FleetSharedSecret},
};

pub struct FleetConfigInternals {
	pub local_system: String,
	pub directory: PathBuf,
	pub opts: FleetOpts,
	pub data: Mutex<FleetData>,
	pub nix_args: Vec<OsString>,
	/// fleet_config.config
	pub config_field: Value,

	/// import nixpkgs {system = local};
	pub default_pkgs: Value,
}

#[derive(Clone)]
pub struct Config(Arc<FleetConfigInternals>);

impl Deref for Config {
	type Target = FleetConfigInternals;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

#[derive(Clone, Copy, Debug)]
pub enum EscalationStrategy {
	Sudo,
	Run0,
	Su,
}

pub struct ConfigHost {
	config: Config,
	pub name: String,
	pub local: bool,
	pub session: OnceLock<Arc<openssh::Session>>,
	groups: OnceCell<Vec<String>>,

	pub host_config: Option<Value>,
	pub nixos_config: OnceCell<Value>,
}
impl ConfigHost {
	pub async fn escalation_strategy(&self) -> Result<EscalationStrategy> {
		// Prefer sudo, as run0 has some gotchas with polkit
		// and too many repeating prompts.
		if let Ok(_) = self.find_in_path("sudo").await {
			return Ok(EscalationStrategy::Sudo);
		}
		if let Ok(_) = self.find_in_path("run0").await {
			return Ok(EscalationStrategy::Run0);
		}
		Ok(EscalationStrategy::Su)
	}
	// TOCTOU is possible here in case if config is changed, but this case is not handled anywhere anyway,
	// assuming getting tags always returns the same value.
	pub async fn tags(&self) -> Result<Vec<String>> {
		if let Some(v) = self.groups.get() {
			return Ok(v.clone());
		}
		let Some(host_config) = &self.host_config else {
			return Ok(vec![]);
		};
		let tags: Vec<String> = nix_go_json!(host_config.tags);

		let _ = self.groups.set(tags.clone());

		Ok(tags)
	}
	pub async fn nixos_config(&self) -> Result<Value> {
		if let Some(v) = self.nixos_config.get() {
			return Ok(v.clone());
		}
		let Some(host_config) = &self.host_config else {
			bail!("local host has no nixos_config");
		};
		let nixos_config = nix_go!(host_config.nixos.config);
		assert_warn("nixos config evaluation", &nixos_config).await?;

		let _ = self.nixos_config.set(nixos_config.clone());

		Ok(nixos_config)
	}
	async fn open_session(&self) -> Result<Arc<openssh::Session>> {
		assert!(!self.local, "do not open ssh connection to local session");
		// FIXME: TOCTOU
		if let Some(session) = &self.session.get() {
			return Ok((*session).clone());
		};
		let mut session = SessionBuilder::default();
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
	pub async fn read_dir(&self, path: impl AsRef<OsStr>) -> Result<Vec<String>> {
		let mut cmd = self.cmd("ls").await?;
		cmd.arg(path);
		let out = cmd.run_string().await?;
		let mut lines = out.split('\n');
		if let Some(last) = lines.next_back() {
			ensure!(last.is_empty(), "output of ls should end with newline");
		}
		Ok(lines.map(ToOwned::to_owned).collect())
	}
	#[allow(dead_code)]
	pub async fn read_file_json<D: DeserializeOwned>(&self, path: impl AsRef<OsStr>) -> Result<D> {
		let text = self.read_file_text(path).await?;
		Ok(serde_json::from_str(&text)?)
	}
	pub async fn read_env(&self, env: &str) -> Result<String> {
		let mut cmd = self.cmd("printenv").await?;
		cmd.arg(env);
		Ok(cmd.run_string().await?)
	}
	pub async fn find_in_path(&self, command: &str) -> Result<String> {
		// // `which` is not a part of coreutils, and it might not exist on machine.
		// let path = self.read_env("PATH").await?;
		// // Assuming delimiter is :, we don't work with windows host, this check will be much
		// // more sophisticated in remowt backend (and quicker, since actual PATH search will be done on remote machine)
		// for ele in path.split(':') {
		// 	let test_path = format!("{ele}/{cmd}");
		// 	test -x etc
		// }
		// let mut cmd = self.cmd("printenv").await?;
		// cmd.arg(env);
		// Ok(cmd.run_string().await?)
		// Assuming this is an environment issue if which doesn't exist, will be fixed with remowt.
		let mut cmd = self
			.cmd_escalation(
				// Not used
				EscalationStrategy::Su,
				"which",
			)
			.await?;
		cmd.arg(command);
		cmd.run_string().await
	}
	pub async fn read_file_value<D: FromStr>(&self, path: impl AsRef<OsStr>) -> Result<D>
	where
		<D as FromStr>::Err: Display,
	{
		let text = self.read_file_text(path).await?;
		D::from_str(&text).map_err(|e| anyhow!("failed to parse value: {e}"))
	}
	pub async fn cmd(&self, cmd: impl AsRef<OsStr>) -> Result<MyCommand> {
		self.cmd_escalation(self.escalation_strategy().await?, cmd)
			.await
	}
	pub async fn cmd_escalation(
		&self,
		escalation: EscalationStrategy,
		cmd: impl AsRef<OsStr>,
	) -> Result<MyCommand> {
		if self.local {
			Ok(MyCommand::new(escalation, cmd))
		} else {
			let session = self.open_session().await?;
			Ok(MyCommand::new_on(escalation, cmd, session))
		}
	}

	pub async fn decrypt(&self, data: SecretData) -> Result<Vec<u8>> {
		ensure!(data.encrypted, "secret is not encrypted");
		let mut cmd = self.cmd("fleet-install-secrets").await?;
		cmd.arg("decrypt").eqarg("--secret", data.to_string());
		let encoded = cmd
			.sudo()
			.run_string()
			.await
			.context("failed to call remote host for decrypt")?;
		let data: SecretData = encoded.parse().map_err(|e| anyhow!("{e}"))?;
		ensure!(!data.encrypted, "secret came out encrypted");
		Ok(data.data)
	}
	pub async fn reencrypt(&self, data: SecretData, targets: Vec<String>) -> Result<SecretData> {
		ensure!(data.encrypted, "secret is not encrypted");
		let mut cmd = self.cmd("fleet-install-secrets").await?;
		cmd.arg("reencrypt").eqarg("--secret", data.to_string());
		for target in targets {
			let key = self.config.key(&target).await?;
			cmd.eqarg("--targets", key);
		}
		let encoded = cmd
			.sudo()
			.run_string()
			.await
			.context("failed to call remote host for decrypt")?;
		let data: SecretData = encoded.parse().map_err(|e| anyhow!("{e}"))?;
		ensure!(data.encrypted, "secret came out not encrypted");
		Ok(data)
	}
	/// Returns path for futureproofing, as path might change i.e on conversion to CA
	pub async fn remote_derivation(&self, path: &PathBuf) -> Result<PathBuf> {
		if self.local {
			// Path is located locally, thus already trusted.
			return Ok(path.to_owned());
		}
		let mut nix = MyCommand::new(
			// Not used
			EscalationStrategy::Su,
			"nix",
		);
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
		let nixos = self.nixos_config().await?;
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
	pub async fn secret_field(&self, name: &str) -> Result<Value> {
		let nixos = self.nixos_config().await?;
		Ok(nix_go!(nixos.secrets[{ name }]))
	}

	/// Packages for this host, resolved with nixpkgs overlays
	pub async fn pkgs(&self) -> Result<Value> {
		let nixos = self.nixos_config().await?;
		Ok(nix_go!(nixos._resolvedPkgs))
	}
}

impl Config {
	pub async fn should_skip(&self, host: &ConfigHost) -> Result<bool> {
		if !self.opts.skip.is_empty() && self.opts.skip.iter().any(|h| h as &str == host.name) {
			return Ok(true);
		}
		if self.opts.only.is_empty() {
			return Ok(false);
		}
		let mut have_group_matches = false;
		for item in self.opts.only.iter() {
			match item {
				HostItem::Host { name, .. } if *name == host.name => {
					return Ok(false);
				}
				HostItem::Tag { .. } => {
					have_group_matches = true;
				}
				_ => {}
			}
		}
		if have_group_matches {
			let host_tags = host.tags().await?;
			for item in self.opts.only.iter() {
				match item {
					HostItem::Tag { name, .. } if host_tags.contains(name) => {
						return Ok(false);
					}
					_ => {}
				}
			}
		}
		Ok(true)
	}
	pub async fn action_attr(&self, host: &ConfigHost, attr: &str) -> Result<Option<String>> {
		if self.opts.only.is_empty() {
			return Ok(None);
		}
		let mut have_group_matches = false;
		for item in self.opts.only.iter() {
			match item {
				HostItem::Host { name, attrs }
					if *name == host.name && attrs.contains_key(attr) =>
				{
					return Ok(attrs.get(attr).cloned());
				}
				HostItem::Tag { attrs, .. } if attrs.contains_key(attr) => {
					have_group_matches = true;
				}
				_ => {}
			}
		}
		if have_group_matches {
			let host_tags = host.tags().await?;
			for item in self.opts.only.iter() {
				match item {
					HostItem::Tag { name, attrs }
						if host_tags.contains(name) && attrs.contains_key(attr) =>
					{
						return Ok(attrs.get(attr).cloned());
					}
					_ => {}
				}
			}
		}
		Ok(None)
	}
	pub fn is_local(&self, host: &str) -> bool {
		self.opts.localhost.as_ref().map(|s| s as &str) == Some(host)
	}

	pub fn local_host(&self) -> ConfigHost {
		ConfigHost {
			config: self.clone(),
			name: "<virtual localhost>".to_owned(),
			local: true,
			session: OnceLock::new(),
			host_config: None,
			nixos_config: OnceCell::new(),
			groups: {
				let cell = OnceCell::new();
				let _ = cell.set(vec![]);
				cell
			},
		}
	}

	pub async fn host(&self, name: &str) -> Result<ConfigHost> {
		let config = &self.config_field;
		let host_config = nix_go!(config.hosts[{ name }]);


		Ok(ConfigHost {
			config: self.clone(),
			name: name.to_owned(),
			local: self.is_local(name),
			session: OnceLock::new(),
			host_config: Some(host_config),
			nixos_config: OnceCell::new(),
			groups: OnceCell::new(),
		})
	}
	pub async fn list_hosts(&self) -> Result<Vec<ConfigHost>> {
		let config = &self.config_field;
		let names = nix_go!(config.hosts).list_fields().await?;
		let mut out = vec![];
		for name in names {
			out.push(self.host(&name).await?);
		}
		Ok(out)
	}
	pub async fn system_config(&self, host: &str) -> Result<Value> {
		let fleet_field = &self.config_field;
		Ok(nix_go!(fleet_field.hosts[{ host }].nixos.config))
	}

	pub(super) fn data(&self) -> MutexGuard<FleetData> {
		self.data.lock().unwrap()
	}
	pub(super) fn data_mut(&self) -> MutexGuard<FleetData> {
		self.data.lock().unwrap()
	}
	/// Shared secrets configured in fleet.nix or in flake
	pub async fn list_configured_shared(&self) -> Result<Vec<String>> {
		let config_field = &self.config_field;
		Ok(nix_go!(config_field.sharedSecrets).list_fields().await?)
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
		let config_field = &self.config_field;
		Ok(nix_go_json!(
			config_field.sharedSecrets[{ secret }].expectedOwners
		))
	}

	pub fn save(&self) -> Result<()> {
		let mut tempfile = NamedTempFile::new_in(self.directory.clone()).context("failed to create updated version of fleet.nix in the same directory as original.\nDo you have write access to it? Access only to the fleet.nix won't be enough, the directory is used for atomic overwrite operation.\nIt is not recommended to use fleet by root anyway, move fleet project to your home directory.")?;
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

#[derive(Clone)]
enum HostItem {
	Host {
		name: String,
		attrs: BTreeMap<String, String>,
	},
	Tag {
		name: String,
		attrs: BTreeMap<String, String>,
	},
}
fn host_item_parser(input: &str) -> Result<HostItem, String> {
	fn err_to_string(err: nom::Err<nom::error::Error<&str>>) -> String {
		err.to_string()
	}

	let (input, is_tag) = map(opt(char('@')), |c| c.is_some())(input).map_err(err_to_string)?;
	let (input, name) = map(
		take_while1(|v| v != ',' && v != '?' && v != '@'),
		str::to_owned,
	)(input)
	.map_err(err_to_string)?;

	let kw_item = separated_pair(
		map(take_while1(|v| v != '&' && v != '='), str::to_owned),
		char('='),
		map(take_while1(|v| v != '&'), str::to_owned),
	);
	let kw = map(separated_list1(char('&'), kw_item), |vec| {
		vec.into_iter().collect::<BTreeMap<_, _>>()
	});
	let mut opt_kw = map(opt(preceded(char('?'), kw)), Option::unwrap_or_default);

	let (input, attrs) = opt_kw(input).map_err(err_to_string)?;

	if !input.is_empty() {
		return Err(format!("unexpected trailing input: {input:?}"));
	}
	Ok(if is_tag {
		HostItem::Tag { name, attrs }
	} else {
		HostItem::Host { name, attrs }
	})
}

#[derive(Parser, Clone)]
pub struct FleetOpts {
	/// All hosts except those would be skipped
	#[clap(long, number_of_values = 1, value_parser = host_item_parser)]
	only: Vec<HostItem>,

	/// Hosts to skip
	#[clap(long, number_of_values = 1)]
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

		let builtins_field = Value::binding(root_field.clone(), "builtins").await?;
		if self.local_system == "detect" {
			self.local_system = nix_go_json!(builtins_field.currentSystem);
		}
		let local_system = self.local_system.clone();

		let mut fleet_data_path = directory.clone();
		fleet_data_path.push("fleet.nix");
		let bytes = std::fs::read_to_string(fleet_data_path)?;
		let data: Mutex<FleetData> = nixlike::parse_str(&bytes)?;

		let fleet_root = Value::binding(root_field, "fleetConfigurations").await?;
		let fleet_field = nix_go!(fleet_root.default({ data }));

		let config_field = nix_go!(fleet_field.config);

		assert_warn("fleet config evaluation", &config_field).await?;

		let import = nix_go!(builtins_field.import);
		let overlays = nix_go!(config_field.nixpkgs.overlays);
		let nixpkgs = nix_go!(fleet_field.nixpkgs.buildUsing | import);

		let default_pkgs = nix_go!(nixpkgs(Obj {
			overlays,
			system: { self.local_system.clone() },
		}));

		Ok(Config(Arc::new(FleetConfigInternals {
			opts: self,
			directory,
			data,
			local_system,
			nix_args,
			config_field,
			default_pkgs,
		})))
	}
}
