use std::{
	env::current_dir,
	ffi::{OsStr, OsString},
	ops::Deref,
	path::PathBuf,
	process::Command,
	sync::Arc,
};

use anyhow::Result;
use clap::Clap;

use crate::command::CommandExt;

pub struct FleetConfigInternals {
	pub directory: PathBuf,
	pub opts: FleetOpts,
}

#[derive(Clone)]
pub struct FleetConfig(Arc<FleetConfigInternals>);

impl Deref for FleetConfig {
	type Target = FleetConfigInternals;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl FleetConfig {
	pub fn data_dir(&self) -> PathBuf {
		let mut out = self.directory.clone();
		out.push(".fleet");
		out
	}

	pub fn full_attr_name(&self, attr_name: &str) -> OsString {
		let mut str = self.directory.as_os_str().to_owned();
		str.push("#");
		str.push(attr_name);
		str
	}

	pub fn list_host_names(&self) -> Result<Vec<String>> {
		Ok(Command::new("nix")
			.arg("eval")
			.arg(self.full_attr_name("fleetConfigurations.default.configuredHosts"))
			.args(&["--apply", "builtins.attrNames", "--json"])
			.inherit_stdio()
			.run_json()?)
	}

	pub fn list_hosts(&self) -> Result<Vec<Host>> {
		Ok(self
			.list_host_names()?
			.into_iter()
			.map(|hostname| Host {
				fleet_config: self.clone(),
				hostname,
			})
			.collect())
	}
}

pub struct Host {
	pub fleet_config: FleetConfig,

	pub hostname: String,
}

impl Host {
	pub fn skip(&self) -> bool {
		self.fleet_config.0.opts.should_skip(&self.hostname)
	}
	pub fn is_local(&self) -> bool {
		self.fleet_config.0.opts.is_local(&self.hostname)
	}
	pub fn command_on(&self, cmd: impl AsRef<OsStr>, sudo: bool) -> Command {
		if !self.is_local() {
			let mut out = Command::new("ssh");
			out.arg(&self.hostname).arg("--");
			if sudo {
				out.arg("sudo");
			}
			out.arg(cmd);
			out
		} else if sudo {
			let mut out = Command::new("sudo");
			out.arg(cmd);
			out
		} else {
			Command::new(cmd)
		}
	}
}

#[derive(Clap, Clone)]
#[clap(group = clap::ArgGroup::new("target_hosts"))]
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
}

impl FleetOpts {
	pub fn should_skip(&self, host: &str) -> bool {
		if self.skip.len() > 0 {
			self.skip.iter().find(|h| h as &str == host).is_some()
		} else if self.only.len() > 0 {
			self.only.iter().find(|h| h as &str == host).is_none()
		} else {
			false
		}
	}
	pub fn is_local(&self, host: &str) -> bool {
		self.localhost.as_ref().map(|s| &s as &str) == Some(host)
	}
	pub fn build(mut self) -> Result<FleetConfig> {
		if self.localhost.is_none() {
			self.localhost
				.replace(hostname::get().unwrap().to_str().unwrap().to_owned());
		}
		let directory = current_dir()?;
		Ok(FleetConfig(Arc::new(FleetConfigInternals {
			opts: self,
			directory,
		})))
	}
}
