pub mod cmds;
pub mod command;
pub mod host;
pub mod keys;

mod fleetdata;

use std::ffi::OsString;
use std::io;

use anyhow::{anyhow, bail, Result};
use clap::Parser;

use cmds::{build_systems::BuildSystems, info::Info, secrets::Secrets};
use host::{Config, FleetOpts};
use tokio::fs;
use tokio::process::Command;
use tracing::{info, metadata::LevelFilter};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
struct Prefetch {}
impl Prefetch {
	async fn run(&self, config: &Config) -> Result<()> {
		let mut prefetch_dir = config.directory.to_path_buf();
		prefetch_dir.push("prefetch");
		if !prefetch_dir.is_dir() {
			info!("nothing to prefetch: no prefetch directory");
			return Ok(());
		}
		for entry in std::fs::read_dir(&prefetch_dir)? {
			let entry = entry?;
			if !entry.metadata()?.is_file() {
				bail!("only files should exist in prefetch directory");
			}
			info!("prefetching {:?}", entry.file_name());
			let mut path = OsString::new();
			path.push("file://");
			path.push(entry.path());
			let status = Command::new("nix-prefetch-url").arg(path).status().await?;
			if !status.success() {
				bail!("failed with {status}");
			}
		}
		Ok(())
	}
}

#[derive(Parser)]
enum Opts {
	/// Prepare systems for deployments
	BuildSystems(BuildSystems),
	/// Secret management
	#[clap(subcommand)]
	Secrets(Secrets),
	/// Upload prefetch directory to the nix store
	Prefetch(Prefetch),
	/// Config parsing
	Info(Info),
}

#[derive(Parser)]
#[clap(version = "1.0", author)]
struct RootOpts {
	#[clap(flatten)]
	fleet_opts: FleetOpts,
	#[clap(subcommand)]
	command: Opts,
}

async fn run_command(config: &Config, command: Opts) -> Result<()> {
	match command {
		Opts::BuildSystems(c) => c.run(config).await?,
		Opts::Secrets(s) => s.run(config).await?,
		Opts::Info(i) => i.run(config).await?,
		Opts::Prefetch(p) => p.run(config).await?,
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
	let mut os_args = std::env::args_os();
	let opts = RootOpts::parse_from((&mut os_args).take_while(|v| v != "--"));
	let config = opts.fleet_opts.build(os_args.collect()).await?;

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
