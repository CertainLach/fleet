#![feature(once_cell)]

pub mod command;
pub mod host;
pub mod keys;

pub mod cmds;
pub mod db;
pub mod nix;

mod fleetdata;

use anyhow::Result;
use clap::Clap;

use cmds::{build_systems::BuildSystems, generate_secrets::GenerateSecrets, secrets::Secrets};
use host::{Config, FleetOpts};

#[derive(Clap)]
#[clap(version = "1.0", author = "CertainLach <iam@lach.pw>")]
enum Opts {
	/// Force generation of missing secrets
	GenerateSecrets(GenerateSecrets),
	/// Prepare systems for deployments
	BuildSystems(BuildSystems),
	/// Secret management
	Secrets(Secrets),
}

#[derive(Clap)]
struct RootOpts {
	#[clap(flatten)]
	fleet_opts: FleetOpts,
	#[clap(subcommand)]
	command: Opts,
}

fn run_command(config: &Config, command: Opts) -> Result<()> {
	match command {
		Opts::BuildSystems(c) => c.run(config)?,
		Opts::GenerateSecrets(c) => c.run()?,
		Opts::Secrets(s) => s.run(config)?,
	};
	Ok(())
}

fn main() -> Result<()> {
	env_logger::Builder::new()
		.filter_level(log::LevelFilter::Info)
		.init();
	let opts = RootOpts::parse();
	let config = opts.fleet_opts.build()?;

	match run_command(&config, opts.command) {
		Ok(()) => {
			config.save()?;
			Ok(())
		}
		Err(e) => {
			let _ = config.save();
			Err(e)
		}
	}
}
