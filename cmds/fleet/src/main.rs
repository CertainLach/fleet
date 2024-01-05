#![recursion_limit = "512"]
#![feature(try_blocks, lint_reasons)]

pub(crate) mod cmds;
pub(crate) mod command;
pub(crate) mod host;
pub(crate) mod keys;

pub(crate) mod better_nix_eval;
pub(crate) mod extra_args;

mod fleetdata;

use std::ffi::OsString;
use std::process::exit;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Parser;

use cmds::{
	build_systems::{BuildSystems, Deploy},
	info::Info,
	secrets::Secret,
};
use futures::future::LocalBoxFuture;
use futures::stream::FuturesUnordered;
use futures::TryStreamExt;
use host::{Config, FleetOpts};
use human_repr::HumanCount;
use indicatif::{ProgressState, ProgressStyle};
use tracing::{error, info};
use tracing::{info_span, Instrument};
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::{prelude::*, EnvFilter};

use crate::command::MyCommand;

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
		let tasks = <FuturesUnordered<LocalBoxFuture<Result<()>>>>::new();
		for entry in std::fs::read_dir(&prefetch_dir)? {
			tasks.push(Box::pin(async {
				let entry = entry?;
				if !entry.metadata()?.is_file() {
					bail!("only files should exist in prefetch directory");
				}
				let span = info_span!(
					"prefetching",
					name = entry.file_name().to_string_lossy().as_ref()
				);
				let mut path = OsString::new();
				path.push("file://");
				path.push(entry.path());

				let mut status = MyCommand::new("nix");
				status.arg("store").arg("prefetch-file").arg(path);
				status.run_nix_string().instrument(span).await?;
				Ok(())
			}));
		}
		tasks.try_collect::<Vec<()>>().await?;
		Ok(())
	}
}

#[derive(Parser)]
enum Opts {
	/// Prepare systems for deployments
	BuildSystems(BuildSystems),

	Deploy(Deploy),
	/// Secret management
	#[clap(subcommand)]
	Secret(Secret),
	/// Upload prefetch directory to the nix store
	Prefetch(Prefetch),
	/// Config parsing
	Info(Info),
}

#[derive(Parser)]
#[clap(version, author)]
struct RootOpts {
	#[clap(flatten)]
	fleet_opts: FleetOpts,
	#[clap(subcommand)]
	command: Opts,
}

async fn run_command(config: &Config, command: Opts) -> Result<()> {
	match command {
		Opts::BuildSystems(c) => c.run(config).await?,
		Opts::Deploy(d) => d.run(config).await?,
		Opts::Secret(s) => s.run(config).await?,
		Opts::Info(i) => i.run(config).await?,
		Opts::Prefetch(p) => p.run(config).await?,
	};
	Ok(())
}

fn setup_logging() {
	let indicatif_layer = IndicatifLayer::new().with_progress_style(
		ProgressStyle::with_template(
			"{color_start}{span_child_prefix} {span_name}{{{span_fields}}}{color_end} {wide_msg} {color_start}{download_progress} {elapsed}{color_end}",
		)
		.unwrap()
		.with_key("download_progress", |state: &ProgressState, writer: &mut dyn std::fmt::Write| {
			let Some(len) = state.len() else {
				return;
			};
			let pos = state.pos();
			let _ = write!(writer, "{} / {}", pos.human_count_bare(), len.human_count_bare());
		})
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

	let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

	tracing_subscriber::registry()
		.with(
			tracing_subscriber::fmt::layer()
				.without_time()
				.with_target(true)
				.with_writer(indicatif_layer.get_stderr_writer())
				.with_filter(filter), // .withou,
		)
		.with(indicatif_layer)
		.init();
}

#[tokio::main]
async fn main() {
	setup_logging();
	if let Err(e) = main_real().await {
		error!("{e:#}");
		exit(1);
	}
}

async fn main_real() -> Result<()> {
	let _ = better_nix_eval::TOKIO_RUNTIME.set(tokio::runtime::Handle::current());

	let nix_args = std::env::var_os("NIX_ARGS")
		.map(|a| extra_args::parse_os(&a))
		.transpose()?
		.unwrap_or_default();
	let opts = RootOpts::parse();
	let config = opts.fleet_opts.build(nix_args).await?;

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
