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
			info!("Building host {}", host);
			let built = tempfile::tempdir()?;
			Command::new("nix")
				.inherit_stdio()
				.arg("build")
				.arg(format!(
					"{}.{}.config.system.build.toplevel",
					SYSTEMS_ATTRIBUTE, host,
				))
				.arg("--no-link")
				.arg("--out-link")
				.arg(built.path())
				.arg("--impure")
				.env("SECRET_DATA", data.clone())
				.run()?;
			info!("Uploading system closure");
			let full_path = std::fs::canonicalize(built.path())?;
			info!("{:?}", full_path);
			Command::new("nix")
				.inherit_stdio()
				.arg("copy")
				.arg(full_path)
				.arg("--to")
				.arg(format!("ssh://root@{}", host))
				.run()?;
			match self.subcommand {
				Some(Subcommand::Test) => {
					info!("Setting system to test")
				}
				Some(Subcommand::Boot) => {
					info!("Setting system to switch on boot")
				}
				Some(Subcommand::Switch) => {
					info!("Switching to configuration")
				}
				_ => {}
			}
		}
		Ok(())
	}
}
