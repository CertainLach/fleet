use std::{
	collections::HashMap,
	ffi::OsStr,
	path::PathBuf,
	process::{Command, Stdio},
};

use anyhow::Result;
use serde::de::DeserializeOwned;

use crate::command::CommandOutput;

pub const HOSTS_ATTRIBUTE: &str = ".#fleetConfigurations.default.configuredHosts";
pub const SECRETS_ATTRIBUTE: &str = ".#fleetConfigurations.default.configuredSecrets";
pub const SYSTEMS_ATTRIBUTE: &str = ".#fleetConfigurations.default.configuredSystems";

pub struct NixCopy {
	closure: PathBuf,
}
impl NixCopy {
	pub fn new(closure: PathBuf) -> Self {
		Self { closure }
	}
	fn run_internal(&self, f: impl Fn(&mut Command)) -> Result<CommandOutput> {
		let mut cmd = Command::new("nix");
		cmd.stderr(Stdio::inherit())
			.arg("copy")
			.arg("--substitute-on-destination")
			.arg(&self.closure);
		f(&mut cmd);

		let out = cmd.output()?;
		if !out.status.success() {
			anyhow::bail!("nix copy failed");
		}
		Ok(CommandOutput(out.stdout))
	}
	pub fn from(&self, from: impl AsRef<OsStr>) -> Result<()> {
		let from = from.as_ref();
		self.run_internal(|cmd| {
			cmd.arg("--from").arg(from);
		})?;
		Ok(())
	}
	pub fn to(&self, to: impl AsRef<OsStr>) -> Result<()> {
		let to = to.as_ref();
		self.run_internal(|cmd| {
			cmd.arg("--to").arg(to);
		})?;
		Ok(())
	}
}

pub struct NixBuild {
	attribute: String,
	impure: bool,
	env: HashMap<String, String>,
}

impl NixBuild {
	pub fn new(attribute: String) -> Self {
		Self {
			attribute,
			impure: false,
			env: HashMap::new(),
		}
	}
	pub fn env(&mut self, name: String, value: String) -> &mut Self {
		self.impure = true;
		self.env.insert(name, value);
		self
	}
	pub fn run(&self) -> Result<tempfile::TempDir> {
		let dir = tempfile::tempdir()?;
		std::fs::remove_dir(dir.path())?;
		let mut cmd = Command::new("nix");
		cmd.stderr(Stdio::inherit())
			.arg("build")
			.arg(&self.attribute)
			.arg("--no-link")
			.arg("--out-link")
			.arg(dir.path());
		if self.impure {
			cmd.arg("--impure");
		}
		if !self.env.is_empty() {
			cmd.envs(&self.env);
		}

		let out = cmd.output()?;
		if !out.status.success() {
			anyhow::bail!("nix eval failed");
		}
		Ok(dir)
	}
}

#[derive(Default)]
pub struct NixEval {
	attribute: String,
	impure: bool,
	apply: Option<String>,
	env: HashMap<String, String>,
}

impl NixEval {
	pub fn new(attribute: String) -> Self {
		Self {
			attribute,
			..Default::default()
		}
	}
	pub fn impure(&mut self) -> &mut Self {
		self.impure = true;
		self
	}
	/// This is the only and impure way to pass something to flake
	/// - https://github.com/NixOS/nix/issues/3949
	/// - https://github.com/NixOS/nixpkgs/issues/101101
	pub fn env(&mut self, name: String, value: String) -> &mut Self {
		self.impure = true;
		self.env.insert(name, value);
		self
	}
	pub fn apply(&mut self, apply: String) -> &mut Self {
		self.apply = Some(apply);
		self
	}
	fn run_internal(&self, f: impl Fn(&mut Command)) -> Result<CommandOutput> {
		let mut cmd = Command::new("nix");
		cmd.stderr(Stdio::inherit())
			.arg("eval")
			.arg("--show-trace")
			.arg(&self.attribute);
		if let Some(apply) = &self.apply {
			cmd.arg("--apply").arg(apply);
		};
		if self.impure {
			cmd.arg("--impure");
		}
		if !self.env.is_empty() {
			cmd.envs(&self.env);
		}
		f(&mut cmd);

		let out = cmd.output()?;
		if !out.status.success() {
			anyhow::bail!("nix eval failed");
		}
		Ok(CommandOutput(out.stdout))
	}
	pub fn run(&self) -> Result<String> {
		Ok(self.run_internal(|_cmd| {})?.as_str()?.to_owned())
	}
	pub fn run_json<T: DeserializeOwned>(&self) -> Result<T> {
		Ok(serde_json::from_slice(
			&self
				.run_internal(|cmd| {
					cmd.arg("--json");
				})?
				.0,
		)?)
	}
	pub fn run_raw(&self) -> Result<String> {
		Ok(self
			.run_internal(|cmd| {
				cmd.arg("--raw");
			})?
			.as_str()?
			.to_owned())
	}
}
