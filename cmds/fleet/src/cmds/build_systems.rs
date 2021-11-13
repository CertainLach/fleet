use std::{env::current_dir, process::Command};

use crate::{command::CommandExt, host::Config, nix::SYSTEMS_ATTRIBUTE};
use anyhow::Result;
use log::info;
use structopt::StructOpt;

#[derive(StructOpt)]
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
	pub fn run(self, config: &Config) -> Result<()> {
		let hosts = config.list_hosts()?;

		for host in hosts.iter() {
			if config.should_skip(host) {
				continue;
			}
			info!("Building host {}", host);
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
				.args(&["build", "--impure", "--no-link", "--out-link"])
				.arg(&built)
				.arg(format!(
					"{}.{}.config.system.build.toplevel",
					SYSTEMS_ATTRIBUTE, host,
				));

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

			nix_build.inherit_stdio().run()?;
			let built = std::fs::canonicalize(built)?;
			info!("Built closure: {:?}", built);

			let action = Action::from(self.subcommand.clone());

			match action {
				Action::Upload(action) => {
					if !config.is_local(host) {
						info!("Uploading system closure");
						Command::new("nix")
							.args(&["copy", "--to"])
							.arg(format!("ssh://root@{}", host))
							.arg(&built)
							.inherit_stdio()
							.run()?;
					}
					if let Some(action) = action {
						if action.should_switch_profile() {
							info!("Switching generation");
							config
								.command_on(host, "nix-env", true)
								.args(&["-p", "/nix/var/nix/profiles/system", "--set"])
								.arg(&built)
								.inherit_stdio()
								.run()?;
						}
						info!("Executing activation script");
						let mut switch_script = built.clone();
						switch_script.push("bin");
						switch_script.push("switch-to-configuration");
						config
							.command_on(host, switch_script, true)
							.arg(action.name())
							.inherit_stdio()
							.run()?;
					}
				}
				Action::Package(PackageAction::SdImage) => {
					let mut out = current_dir()?;
					out.push(format!("sd-image-{}", host));

					info!("Building sd image to {:?}", out);
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
						.arg(format!(
							"{}.{}.config.system.build.sdImage",
							SYSTEMS_ATTRIBUTE, host,
						));
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

					nix_build.inherit_stdio().run()?;
				}
			};
		}
		Ok(())
	}
}
