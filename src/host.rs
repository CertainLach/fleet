use std::{
	cell::{Ref, RefCell, RefMut},
	env::current_dir,
	ffi::{OsStr, OsString},
	ops::Deref,
	path::PathBuf,
	process::Command,
	sync::Arc,
};

use anyhow::Result;
use clap::Clap;

use crate::{command::CommandExt, fleetdata::FleetData};

pub struct FleetConfigInternals {
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

	pub fn full_attr_name(&self, attr_name: &str) -> OsString {
		let mut str = self.directory.as_os_str().to_owned();
		str.push("#");
		str.push(attr_name);

		println!("{:?}", str);
		str
	}

	pub fn list_hosts(&self) -> Result<Vec<String>> {
		Command::new("nix")
			.arg("eval")
			.arg(self.full_attr_name("fleetConfigurations.default.configuredHosts"))
			.args(&["--apply", "builtins.attrNames", "--json", "--show-trace"])
			.inherit_stdio()
			.run_json()
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
	pub fn build(mut self) -> Result<Config> {
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
		})))
	}
}
