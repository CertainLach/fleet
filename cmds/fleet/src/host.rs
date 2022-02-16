use std::{
	cell::{Ref, RefCell, RefMut},
	env::current_dir,
	ffi::{OsStr, OsString},
	ops::Deref,
	path::PathBuf,
	sync::Arc,
};

use anyhow::Result;
use serde::de::DeserializeOwned;
use structopt::clap::ArgGroup;
use structopt::StructOpt;
use tokio::process::Command;

use crate::{command::CommandExt, fleetdata::FleetData};

pub struct FleetConfigInternals {
	pub local_system: String,
	pub directory: PathBuf,
	pub opts: FleetOpts,
	pub data: RefCell<FleetData>,
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
			.args(&["--apply", "builtins.attrNames", "--json", "--show-trace"])
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
			.args(&["--json", "--show-trace"])
			.run_nix_json()
			.await
	}

	pub fn data(&self) -> Ref<FleetData> {
		self.data.borrow()
	}
	pub fn data_mut(&self) -> RefMut<FleetData> {
		self.data.borrow_mut()
	}

	pub fn save(&self) -> Result<()> {
		let mut fleet_data_path = self.directory.clone();
		fleet_data_path.push("fleet.nix");
		let data = nixlike::serialize(&self.data() as &FleetData)?;
		std::fs::write(
			fleet_data_path,
			format!(
				"# This file contains fleet state and shouldn't be edited by hand\n\n{}\n",
				data
			),
		)?;
		Ok(())
	}
}

#[derive(StructOpt, Clone)]
#[structopt(group = ArgGroup::with_name("target_hosts"))]
pub struct FleetOpts {
	/// All hosts except those would be skipped
	#[structopt(long, number_of_values = 1, group = "target_hosts")]
	only: Vec<String>,

	/// Hosts to skip
	#[structopt(long, number_of_values = 1, group = "target_hosts")]
	skip: Vec<String>,

	/// Host, which should be threaten as current machine
	#[structopt(long)]
	pub localhost: Option<String>,

	#[structopt(long, default_value = "x86_64-linux")]
	pub local_system: String,
}

impl FleetOpts {
	pub fn build(mut self) -> Result<Config> {
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
		})))
	}
}
