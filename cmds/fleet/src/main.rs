#![feature(try_blocks)]

pub mod cmds;
pub mod command;
pub mod host;
pub mod keys;

mod fleetdata;

use std::ffi::OsString;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Parser;

use cmds::{build_systems::BuildSystems, info::Info, secrets::Secrets};
use host::{Config, FleetOpts};
use indicatif::{ProgressState, ProgressStyle};
use tokio::process::Command;
use tracing::{info, metadata::LevelFilter};
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::{prelude::*, EnvFilter};

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
	let indicatif_layer = IndicatifLayer::new().with_progress_style(
		ProgressStyle::with_template(
			"{color_start}{span_child_prefix} {span_name}{{{span_fields}}}{color_end} {wide_msg} {color_start}{pos:>7}/{len:7}{elapsed}{color_end}",
		)
		.unwrap()
		.with_key(
			"color_start",
			|state: &ProgressState, writer: &mut dyn std::fmt::Write| {
				let elapsed = state.elapsed();

				if elapsed > Duration::from_secs(60) {
					// Red
					let _ = write!(writer, "\x1b[{}m", 1 + 30);
				} else if elapsed > Duration::from_secs(30) {
					// Yellow
					let _ = write!(writer, "\x1b[{}m", 3 + 30);
				}
			},
		)
		.with_key(
			"color_end",
			|state: &ProgressState, writer: &mut dyn std::fmt::Write| {
				if state.elapsed() > Duration::from_secs(30) {
					let _ = write!(writer, "\x1b[0m");
				}
			},
		),
	);

	let filter = EnvFilter::from_default_env().add_directive(LevelFilter::INFO.into());

	tracing_subscriber::registry()
		.with(
			tracing_subscriber::fmt::layer()
				.without_time()
				.with_target(false)
				.with_writer(indicatif_layer.get_stderr_writer())
				.with_filter(filter), // .withou,
		)
		.with(indicatif_layer)
		.init();
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
