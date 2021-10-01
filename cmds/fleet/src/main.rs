pub mod command;
pub mod host;
pub mod keys;

pub mod cmds;
pub mod nix;

mod fleetdata;

use anyhow::Result;
use structopt::clap::AppSettings::*;
use structopt::StructOpt;

use cmds::{build_systems::BuildSystems, info::Info, secrets::Secrets};
use host::{Config, FleetOpts};

#[derive(StructOpt)]
enum Opts {
	/// Prepare systems for deployments
	BuildSystems(BuildSystems),
	/// Secret management
	Secrets(Secrets),
	/// Config parsing
	Info(Info),
}

#[derive(StructOpt)]
#[structopt(
	version = "1.0",
	author,
	global_setting(ColorAuto),
	global_setting(ColoredHelp)
)]
struct RootOpts {
	#[structopt(flatten)]
	fleet_opts: FleetOpts,
	#[structopt(subcommand)]
	command: Opts,
}

fn run_command(config: &Config, command: Opts) -> Result<()> {
	match command {
		Opts::BuildSystems(c) => c.run(config)?,
		Opts::Secrets(s) => s.run(config)?,
		Opts::Info(i) => i.run(config)?,
	};
	Ok(())
}

fn main() -> Result<()> {
	env_logger::Builder::new()
		.filter_level(log::LevelFilter::Info)
		.init();
	let opts = RootOpts::from_args();
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
