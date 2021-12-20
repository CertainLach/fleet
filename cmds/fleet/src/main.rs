#![feature(try_blocks)]

pub mod command;
pub mod host;
pub mod keys;

pub mod cmds;
pub mod nix;

mod fleetdata;

use std::io;

use anyhow::{anyhow, Result};
use structopt::clap::AppSettings::*;
use structopt::StructOpt;

use cmds::{build_systems::BuildSystems, info::Info, secrets::Secrets};
use host::{Config, FleetOpts};
use tracing::{info, metadata::LevelFilter};
use tracing_subscriber::EnvFilter;

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

async fn run_command(config: &Config, command: Opts) -> Result<()> {
	match command {
		Opts::BuildSystems(c) => c.run(config).await?,
		Opts::Secrets(s) => s.run(config).await?,
		Opts::Info(i) => i.run(config).await?,
	};
	Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
	let filter = EnvFilter::from_default_env().add_directive(LevelFilter::INFO.into());
	tracing_subscriber::FmtSubscriber::builder()
		.with_env_filter(filter)
		.without_time()
		.with_target(false)
		.with_writer(|| {
			// eprintln!("Line");
			io::stderr()
		})
		.try_init()
		.map_err(|e| anyhow!("Failed to initialize logger: {}", e))?;

	info!("Starting");
	let opts = RootOpts::from_args();
	let config = opts.fleet_opts.build()?;

	match run_command(&config, opts.command).await {
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
