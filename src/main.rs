#![feature(once_cell)]

pub mod command;
pub mod host;
pub mod keys;

pub mod cmds;
pub mod db;
pub mod nix;

use anyhow::Result;
use clap::Clap;
use cmds::{build_systems::BuildSystems, fetch_keys::FetchKeys, generate_secrets::GenerateSecrets};

#[derive(Clap)]
#[clap(version = "1.0", author = "CertainLach <iam@lach.pw>")]
enum Opts {
	/// Fetch encryption (ssh) public keys from remote hosts
	FetchKeys(FetchKeys),
	/// Force generation of missing secrets
	GenerateSecrets(GenerateSecrets),
	/// Prepare systems for deployments
	BuildSystems(BuildSystems),
}

fn main() -> Result<()> {
	env_logger::Builder::new()
		.filter_level(log::LevelFilter::Info)
		.init();
	let opts = Opts::parse();

	match opts {
		Opts::FetchKeys(c) => c.run()?,
		Opts::BuildSystems(c) => c.run()?,
		Opts::GenerateSecrets(c) => c.run()?,
	};
	Ok(())
}
