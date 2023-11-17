#![feature(try_blocks)]

pub(crate) mod cmds;
pub(crate) mod command;
pub(crate) mod host;
pub(crate) mod keys;

pub(crate) mod better_nix_eval;
pub(crate) mod extra_args;

mod fleetdata;

use std::ffi::OsString;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Parser;

use cmds::{build_systems::BuildSystems, info::Info, secrets::Secrets};
use futures::future::LocalBoxFuture;
use futures::stream::FuturesUnordered;
use futures::TryStreamExt;
use host::{Config, FleetOpts};
use human_repr::HumanCount;
use indicatif::{ProgressState, ProgressStyle};
use tracing::{info, metadata::LevelFilter};
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

// fn main() -> Result<()> {
// 	let pool = r2d2::Builder::<NixSessionPool>::new()
// 		.min_idle(Some(1))
// 		.max_lifetime(Some(Duration::from_secs(10)))
// 		.build(NixSessionPool {
// 			flake: ".".to_owned(),
// 			nix_args: vec![],
// 		})?;
// 	let conn = pool.get()?;
// 	let field = Field::root(conn);
// 	// let builtins = field.get_field("builtins")?;
// 	let cur_sys: String = field.get_field("builtins")?.as_json()?;
// 	eprintln!("current system = {cur_sys}");
// 	let v = field.get_field("fleetConfigurations")?;
// 	eprintln!("configs = {:?}", v.list_fields()?);
// 	let d = v.get_field("default")?;
// 	dbg!(d.list_fields());
// 	Ok(())
// }
//

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
}

#[tokio::main]
async fn main() -> Result<()> {
	setup_logging();
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
