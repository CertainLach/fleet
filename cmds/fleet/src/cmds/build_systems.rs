use std::process::Command;

use crate::{command::CommandExt, host::Config, nix::SYSTEMS_ATTRIBUTE};
use anyhow::Result;
use log::info;
use structopt::StructOpt;

#[derive(StructOpt)]
pub struct BuildSystems {
	/// --builders arg for nix
	#[structopt(long)]
	builders: Option<String>,
	/// Jobs to run locally
	#[structopt(long)]
	jobs: Option<usize>,
	/// Do not continue on error
	#[structopt(long)]
	fail_fast: bool,
	#[structopt(long)]
	privileged_build: bool,
	#[structopt(subcommand)]
	subcommand: Option<Subcommand>,
}

#[derive(StructOpt)]
enum Subcommand {
	/// Switch to built system until reboot
	Test,
	/// Switch to built system after reboot
	Boot,
	/// test + boot
	Switch,
}
impl Subcommand {
	fn should_switch_profile(&self) -> bool {
		matches!(self, Self::Test | Self::Switch)
	}
	fn name(&self) -> &'static str {
		match self {
			Self::Test => "test",
			Self::Boot => "boot",
			Self::Switch => "switch",
		}
	}
}

impl BuildSystems {
	pub fn run(self, config: &Config) -> Result<()> {
		let hosts = config.list_hosts()?;

		for host in hosts.iter() {
			if config.should_skip(host) {
				continue;
			}
			info!("Building host {}", host);
			let built = {
				let dir = tempfile::tempdir()?;
				dir.path().to_owned()
			};

			let mut nix_build = if self.privileged_build {
				let mut out = Command::new("sudo");
				out.arg("nix");
				out
			} else {
				Command::new("nix")
			};
			nix_build
				.args(&["build", "--impure", "--no-link", "--out-link"])
				.arg(&built)
				.arg(format!(
					"{}.{}.config.system.build.toplevel",
					SYSTEMS_ATTRIBUTE, host,
				));

			if let Some(builders) = &self.builders {
				nix_build.arg("--builders").arg(builders);
			}
			if let Some(jobs) = &self.jobs {
				nix_build.arg("--max-jobs");
				nix_build.arg(format!("{}", jobs));
			}
			if !self.fail_fast {
				nix_build.arg("--keep-going");
			}

			nix_build.inherit_stdio().run()?;
			let built = std::fs::canonicalize(built)?;
			info!("Built closure: {:?}", built);
			if !config.is_local(host) {
				info!("Uploading system closure");
				Command::new("nix")
					.args(&["copy", "--to"])
					.arg(format!("ssh://root@{}", host))
					.arg(&built)
					.inherit_stdio()
					.run()?;
			}
			if let Some(subcommand) = &self.subcommand {
				if subcommand.should_switch_profile() {
					info!("Switching generation");
					config
						.command_on(host, "nix-env", true)
						.args(&["-p", "/nix/var/nix/profiles/system", "--set"])
						.arg(&built)
						.inherit_stdio()
						.run()?;
				}
				info!("Executing activation script");
				let mut switch_script = built.clone();
				switch_script.push("bin");
				switch_script.push("switch-to-configuration");
				config
					.command_on(host, switch_script, true)
					.arg(subcommand.name())
					.inherit_stdio()
					.run()?;
			}
		}
		Ok(())
	}
}
