use std::process::Command;

use crate::{
	command::CommandExt,
	db::{keys::list_hosts, secret::SecretDb, Db, DbData},
	nix::SYSTEMS_ATTRIBUTE,
};
use anyhow::Result;
use clap::Clap;
use log::{info, warn};

#[derive(Clap)]
pub struct BuildSystems {
	/// Hosts to skip
	#[clap(long, number_of_values = 1)]
	skip: Vec<String>,
	/// Host, which should be threaten as localhost
	#[clap(long, env = "FLEET_LOCALHOST")]
	localhost: Option<String>,
	/// --builders arg for nix
	#[clap(long)]
	builders: Option<String>,
	/// Jobs to run locally
	#[clap(long)]
	jobs: Option<usize>,
	/// Do not continue on error
	#[clap(long)]
	fail_fast: bool,
	#[clap(subcommand)]
	subcommand: Option<Subcommand>,
}

#[derive(Clap)]
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
	pub fn run(self) -> Result<()> {
		let db = Db::new(".fleet")?;
		let hosts = list_hosts()?;
		let data = SecretDb::open(&db)?.generate_nix_data()?;

		for host in hosts.iter() {
			if self.skip.contains(host) {
				warn!("Skipping host {}", host);
				continue;
			}
			let is_local = Some(host) == self.localhost.as_ref();
			info!("Building host {}", host);
			let built = {
				let dir = tempfile::tempdir()?;
				dir.path().to_owned()
			};

			let mut nix_build = Command::new("nix");
			nix_build
				.args(&["build", "--impure", "--no-link", "--out-link"])
				.arg(&built)
				.arg(format!(
					"{}.{}.config.system.build.toplevel",
					SYSTEMS_ATTRIBUTE, host,
				))
				.env("SECRET_DATA", data.clone());

			if let Some(builders) = &self.builders {
				println!("Using builders: {}", builders);
				nix_build.arg("--builders").arg(builders);
			}
			if let Some(jobs) = &self.jobs {
				nix_build.arg("--jobs");
				nix_build.arg(format!("{}", jobs));
			}
			if !self.fail_fast {
				nix_build.arg("--keep-going");
			}

			nix_build.inherit_stdio().run()?;
			let built = std::fs::canonicalize(built)?;
			info!("Built closure: {:?}", built);
			if !is_local {
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
					if !is_local {
						Command::ssh_on(host, "sudo")
					} else {
						Command::new("sudo")
					}
					.args(&["nix-env", "-p", "/nix/var/nix/profiles/system", "--set"])
					.arg(&built)
					.inherit_stdio()
					.run()?;
				}
				info!("Executing activation script");
				let mut switch_script = built.clone();
				switch_script.push("bin");
				switch_script.push("switch-to-configuration");
				info!("{:?}", switch_script);
				if !is_local {
					Command::ssh_on(host, "sudo")
				} else {
					Command::new("sudo")
				}
				.arg(switch_script)
				.arg(subcommand.name())
				.inherit_stdio()
				.run()?;
			}
		}
		Ok(())
	}
}
