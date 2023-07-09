use std::{env::current_dir, time::Duration};

use crate::command::MyCommand;
use crate::host::Config;
use anyhow::Result;
use clap::Parser;
use tokio::{task::LocalSet, time::sleep};
use tracing::{error, field, info, info_span, warn, Instrument};

#[derive(Parser, Clone)]
pub struct BuildSystems {
	/// Do not continue on error
	#[clap(long)]
	fail_fast: bool,
	/// Run builds as sudo
	#[clap(long)]
	privileged_build: bool,
	#[clap(subcommand)]
	subcommand: Subcommand,
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
		matches!(self, Self::Switch | Self::Boot)
	}
	pub(crate) fn should_activate(&self) -> bool {
		matches!(self, Self::Switch | Self::Test)
	}
}

enum PackageAction {
	SdImage,
	InstallationCd,
}
impl PackageAction {
	fn build_attr(&self) -> String {
		match self {
			PackageAction::SdImage => "sdImage".to_owned(),
			PackageAction::InstallationCd => "installationCd".to_owned(),
		}
	}
}

enum Action {
	Upload { action: Option<UploadAction> },
	Package(PackageAction),
}
impl Action {
	fn build_attr(&self) -> String {
		match self {
			Action::Upload { .. } => "toplevel".to_owned(),
			Action::Package(p) => p.build_attr(),
		}
	}
}

impl From<Subcommand> for Action {
	fn from(s: Subcommand) -> Self {
		match s {
			Subcommand::Upload => Self::Upload { action: None },
			Subcommand::Test => Self::Upload {
				action: Some(UploadAction::Test),
			},
			Subcommand::Boot => Self::Upload {
				action: Some(UploadAction::Boot),
			},
			Subcommand::Switch => Self::Upload {
				action: Some(UploadAction::Switch),
			},
			Subcommand::SdImage => Self::Package(PackageAction::SdImage),
			Subcommand::InstallationCd => Self::Package(PackageAction::InstallationCd),
		}
	}
}

#[derive(Parser, Clone)]
enum Subcommand {
	/// Upload, but do not switch
	Upload,
	/// Upload + switch to built system until reboot
	Test,
	/// Upload + switch to built system after reboot
	Boot,
	/// Upload + test + boot
	Switch,

	/// Build SD .img image
	SdImage,
	/// Build an installation cd ISO image
	InstallationCd,
}

impl BuildSystems {
	async fn build_task(self, config: Config, host: String) -> Result<()> {
		info!("building");
		let action = Action::from(self.subcommand.clone());
		let built = {
			let dir = tempfile::tempdir()?;
			dir.path().to_owned()
		};

		let mut nix_build = MyCommand::new("nix");
		nix_build
			.args([
				"build",
				"--impure",
				"--json",
				// "--show-trace",
				"--no-link",
				"--option",
				"log-lines",
				"200",
			])
			.comparg("--out-link", &built)
			.arg(
				config.configuration_attr_name(&format!(
					"buildSystems.{}.{host}",
					action.build_attr()
				)),
			)
			.args(&config.nix_args);

		if self.privileged_build {
			nix_build = nix_build.sudo();
		}

		nix_build.run_nix().await.map_err(|e| {
			if action.build_attr() == "sdImage" {
				info!("sd-image build failed");
				info!("Make sure you have imported modulesPath/installer/sd-card/sd-image-<arch>[-installer].nix (For installer, you may want to check config)");
				info!("This module was automatically imported before, but was removed for better customization")
			}
			e
		})?;
		let built = std::fs::canonicalize(built)?;

		match action {
			Action::Upload { action } => {
				if !config.is_local(&host) {
					info!("uploading system closure");
					let mut tries = 0;
					loop {
						let mut nix = MyCommand::new("nix");
						nix.arg("copy")
							.comparg("--to", format!("ssh://root@{host}"))
							.arg(&built);
						match nix.run_nix().await {
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
						let mut cmd = MyCommand::new("nix-env");
						cmd.comparg("--profile", "/nix/var/nix/profiles/system")
							.comparg("--set", &built);
						config.run_on(&host, cmd, true).await?;
					}
					if action.should_activate() {
						info!("executing activation script");
						let mut switch_script = built.clone();
						switch_script.push("bin");
						switch_script.push("switch-to-configuration");
						let mut cmd = MyCommand::new(switch_script);
						cmd.arg(action.name());
						config.run_on(&host, cmd, true).await?;
					}
				}
			}
			Action::Package(PackageAction::SdImage) => {
				let mut out = current_dir()?;
				out.push(format!("sd-image-{}", host));

				info!("building sd image to {:?}", out);
				let mut nix_build = MyCommand::new("nix");
				nix_build
					.args(["build", "--impure", "--no-link"])
					.comparg("--out-link", &out)
					.arg(config.configuration_attr_name(&format!("buildSystems.sdImage.{}", host,)))
					.args(&config.nix_args);
				if !self.fail_fast {
					nix_build.arg("--keep-going");
				}
				if self.privileged_build {
					nix_build = nix_build.sudo();
				}

				nix_build.run_nix().await?;
			}
			Action::Package(PackageAction::InstallationCd) => {
				let mut out = current_dir()?;
				out.push(format!("installation-cd-{}", host));

				info!("building sd image to {:?}", out);
				let mut nix_build = MyCommand::new("nix");
				nix_build
					.args(["build", "--impure", "--no-link"])
					.comparg("--out-link", &out)
					.arg(
						config.configuration_attr_name(&format!(
							"buildSystems.installationCd.{}",
							host,
						)),
					)
					.args(&config.nix_args);
				if !self.fail_fast {
					nix_build.arg("--keep-going");
				}
				if self.privileged_build {
					nix_build = nix_build.sudo();
				}

				nix_build.run_nix().await?;
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
