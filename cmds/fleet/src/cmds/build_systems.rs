use std::{env::current_dir, time::Duration};

use crate::{command::CommandExt, host::Config};
use anyhow::Result;
use structopt::StructOpt;
use tokio::{process::Command, task::LocalSet, time::sleep};
use tracing::{error, field, info, info_span, warn, Instrument};

#[derive(StructOpt, Clone)]
pub struct BuildSystems {
	/// --builders arg for nix
	#[structopt(long)]
	builders: Option<String>,
	/// Jobs to run locally
	#[structopt(long)]
	jobs: Option<usize>,
	/// Do not continue on error
	#[structopt(long)]
	fail_fast: bool,
	#[structopt(long)]
	privileged_build: bool,
	#[structopt(subcommand)]
	subcommand: Subcommand,
	#[structopt(long)]
	show_trace: bool,
}

enum UploadAction {
	Test,
	Boot,
	Switch,
}
impl UploadAction {
	fn name(&self) -> &'static str {
		match self {
			UploadAction::Test => "test",
			UploadAction::Boot => "boot",
			UploadAction::Switch => "switch",
		}
	}

	pub(crate) fn should_switch_profile(&self) -> bool {
		matches!(self, Self::Switch | Self::Test)
	}
}

enum PackageAction {
	SdImage,
}

enum Action {
	Upload(Option<UploadAction>),
	Package(PackageAction),
}

impl From<Subcommand> for Action {
	fn from(s: Subcommand) -> Self {
		match s {
			Subcommand::Upload => Self::Upload(None),
			Subcommand::Test => Self::Upload(Some(UploadAction::Test)),
			Subcommand::Boot => Self::Upload(Some(UploadAction::Boot)),
			Subcommand::Switch => Self::Upload(Some(UploadAction::Switch)),
			Subcommand::SdImage => Self::Package(PackageAction::SdImage),
		}
	}
}

#[derive(StructOpt, Clone)]
enum Subcommand {
	/// Upload, but do not switch
	Upload,
	/// Upload + switch to built system until reboot
	Test,
	/// Upload + switch to built system after reboot
	Boot,
	/// Upload + test + boot
	Switch,

	/// Build sd image
	SdImage,
}

impl BuildSystems {
	async fn build_task(self, config: Config, host: String) -> Result<()> {
		info!("building");
		let built = {
			let dir = tempfile::tempdir()?;
			dir.path().to_owned()
		};

		let mut nix_build = if self.privileged_build {
			let mut out = Command::new("sudo");
			out.arg("nix");
			out
		} else {
			Command::new("nix")
		};
		nix_build
			.args(&[
				"build",
				"--impure",
				"--json",
				// "--show-trace",
				"--no-link",
				"--out-link",
			])
			.arg(&built)
			.arg(config.configuration_attr_name(&format!(
				"configuredSystems.{}.config.system.build.toplevel",
				host
			)));

		if self.show_trace {
			nix_build.arg("--show-trace");
		}
		if let Some(builders) = &self.builders {
			nix_build.arg("--builders").arg(builders);
		}
		if let Some(jobs) = &self.jobs {
			nix_build.arg("--max-jobs");
			nix_build.arg(format!("{}", jobs));
		}
		if !self.fail_fast {
			nix_build.arg("--keep-going");
		}

		nix_build.run_nix().await?;
		let built = std::fs::canonicalize(built)?;

		let action = Action::from(self.subcommand.clone());

		match action {
			Action::Upload(action) => {
				if !config.is_local(&host) {
					info!("uploading system closure");
					let mut tries = 0;
					loop {
						match Command::new("nix")
							.args(&["copy", "--to"])
							.arg(format!("ssh://root@{}", host))
							.arg(&built)
							.inherit_stdio()
							.run_nix()
							.await
						{
							Ok(()) => break,
							Err(e) if tries < 3 => {
								tries += 1;
								warn!("Copy failure ({}/3): {}", tries, e);
								sleep(Duration::from_millis(5000)).await;
							}
							Err(e) => return Err(e),
						}
					}
				}
				if let Some(action) = action {
					if action.should_switch_profile() {
						info!("switching generation");
						config
							.command_on(&host, "nix-env", true)
							.args(&["-p", "/nix/var/nix/profiles/system", "--set"])
							.arg(&built)
							.inherit_stdio()
							.run()
							.await?;
					}
					info!("executing activation script");
					let mut switch_script = built.clone();
					switch_script.push("bin");
					switch_script.push("switch-to-configuration");
					config
						.command_on(&host, switch_script, true)
						.arg(action.name())
						.inherit_stdio()
						.run()
						.await?;
				}
			}
			Action::Package(PackageAction::SdImage) => {
				let mut out = current_dir()?;
				out.push(format!("sd-image-{}", host));

				info!("building sd image to {:?}", out);
				let mut nix_build = if self.privileged_build {
					let mut out = Command::new("sudo");
					out.arg("nix");
					out
				} else {
					Command::new("nix")
				};
				nix_build
					.args(&["build", "--impure", "--no-link", "--out-link"])
					.arg(&out)
					.arg(config.configuration_attr_name(&format!(
						"configuredSystems.{}.config.system.build.sdImage",
						host,
					)));
				if let Some(builders) = &self.builders {
					nix_build.arg("--builders").arg(builders);
				}
				if let Some(jobs) = &self.jobs {
					nix_build.arg("--max-jobs");
					nix_build.arg(format!("{}", jobs));
				}
				if !self.fail_fast {
					nix_build.arg("--keep-going");
				}

				nix_build.inherit_stdio().run_nix().await?;
			}
		};
		Ok(())
	}

	pub async fn run(self, config: &Config) -> Result<()> {
		let hosts = config.list_hosts().await?;
		let set = LocalSet::new();
		let this = &self;
		for host in hosts.iter() {
			if config.should_skip(host) {
				continue;
			}
			let config = config.clone();
			let host = host.clone();
			let this = this.clone();
			let span = info_span!("deployment", host = field::display(&host));
			set.spawn_local(
				(async move {
					match this.build_task(config, host).await {
						Ok(_) => {}
						Err(e) => {
							error!("failed to deploy host: {}", e)
						}
					}
				})
				.instrument(span),
			);
		}
		set.await;
		Ok(())
	}
}
