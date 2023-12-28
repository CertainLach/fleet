use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::{env::current_dir, time::Duration};

use crate::command::MyCommand;
use crate::host::Config;
use crate::nix_go;
use anyhow::{anyhow, Result};
use clap::Parser;
use itertools::Itertools;
use tokio::{task::LocalSet, time::sleep};
use tracing::{error, field, info, info_span, warn, Instrument};

#[derive(Parser, Clone)]
pub struct BuildSystems {
	/// Disable automatic rollback
	#[clap(long)]
	disable_rollback: bool,
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
	pub(crate) fn should_schedule_rollback_run(&self) -> bool {
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

struct Generation {
	id: u32,
	current: bool,
	datetime: String,
}
async fn get_current_generation(config: &Config, host: &str) -> Result<Generation> {
	let mut cmd = MyCommand::new("nix-env");
	cmd.comparg("--profile", "/nix/var/nix/profiles/system")
		.arg("--list-generations");
	// Sudo is required due to --list-generations acquiring lock on the profile.
	let data = config.run_string_on(host, cmd, true).await?;
	let generations = data
		.split('\n')
		.map(|e| e.trim())
		.filter(|&l| !l.is_empty())
		.filter_map(|g| {
			let gen: Option<Generation> = try {
				let mut parts = g.split_whitespace();
				let id = parts.next()?;
				let id: u32 = id.parse().ok()?;
				let date = parts.next()?;
				let time = parts.next()?;
				let current = if let Some(current) = parts.next() {
					if current == "(current)" {
						Some(true)
					} else {
						None
					}
				} else {
					Some(false)
				};
				let current = current?;
				if parts.next().is_some() {
					warn!("unexpected text after generation: {g}");
				}
				Generation {
					id,
					current,
					datetime: format!("{date} {time}"),
				}
			};
			if gen.is_none() {
				warn!("bad generation: {g}")
			}
			gen
		})
		.collect::<Vec<_>>();
	let current = generations
		.into_iter()
		.filter(|g| g.current)
		.at_most_one()
		.map_err(|_e| anyhow!("bad list-generations output"))?
		.ok_or_else(|| anyhow!("failed to find generation"))?;
	Ok(current)
}

async fn systemctl_stop(config: &Config, host: &str, unit: &str) -> Result<()> {
	let mut cmd = MyCommand::new("systemctl");
	cmd.arg("stop").arg(unit);
	config.run_on(host, cmd, true).await
}

async fn systemctl_start(config: &Config, host: &str, unit: &str) -> Result<()> {
	let mut cmd = MyCommand::new("systemctl");
	cmd.arg("start").arg(unit);
	config.run_on(host, cmd, true).await
}

async fn execute_upload(
	build: &BuildSystems,
	config: &Config,
	action: UploadAction,
	host: &str,
	built: PathBuf,
) -> Result<()> {
	let mut failed = false;
	// TODO: Lockfile, to prevent concurrent system switch?
	// TODO: If rollback target exists - bail, it should be removed. Lockfile will not work in case if rollback
	// is scheduler on next boot (default behavior). On current boot - rollback activator will fail due to
	// unit name conflict in systemd-run
	// This code is tied to rollback.nix
	if !build.disable_rollback {
		let _span = info_span!("preparing").entered();
		info!("preparing for rollback");
		let generation = get_current_generation(config, host).await?;
		info!(
			"rollback target would be {} {}",
			generation.id, generation.datetime
		);
		{
			let mut cmd = MyCommand::new("sh");
			cmd.arg("-c").arg(format!("mark=$(mktemp -p /etc -t fleet_rollback_marker.XXXXX) && echo -n {} > $mark && mv --no-clobber $mark /etc/fleet_rollback_marker", generation.id));
			if let Err(e) = config.run_on(host, cmd, true).await {
				error!("failed to set rollback marker: {e}");
				failed = true;
			}
		}
		// Activation script also starts rollback-watchdog.timer, however, it is possible that it won't be started.
		// Kicking it on manually will work best.
		//
		// There wouldn't be conflict, because here we trigger start of the primary service, and systemd will
		// only allow one instance of it.

		// TODO: We should also watch how this process is going.
		// After running this command, we have less than 3 minutes to deploy everything,
		// if we fail to perform generation switch in time, then we will still call the activation script, and this may break something.
		// Anyway, reboot will still help in this case.
		if action.should_schedule_rollback_run() {
			let mut cmd = MyCommand::new("systemd-run");
			cmd.comparg("--on-active", "3min")
				.comparg("--unit", "rollback-watchdog-run")
				.arg("systemctl")
				.arg("start")
				.arg("rollback-watchdog.service");
			if let Err(e) = config.run_on(host, cmd, true).await {
				error!("failed to schedule rollback run: {e}");
				failed = true;
			}
		}
	}
	if action.should_switch_profile() && !failed {
		info!("switching generation");
		let mut cmd = MyCommand::new("nix-env");
		cmd.comparg("--profile", "/nix/var/nix/profiles/system")
			.comparg("--set", &built);
		if let Err(e) = config.run_on(host, cmd, true).await {
			error!("failed to switch generation: {e}");
			failed = true;
		}
	}
	if action.should_activate() && !failed {
		let _span = info_span!("activating").entered();
		info!("executing activation script");
		let mut switch_script = built.clone();
		switch_script.push("bin");
		switch_script.push("switch-to-configuration");
		let mut cmd = MyCommand::new(switch_script);
		cmd.arg(action.name());
		if let Err(e) = config.run_on(host, cmd, true).in_current_span().await {
			error!("failed to activate: {e}");
			failed = true;
		}
	}
	if !build.disable_rollback {
		if failed {
			info!("executing rollback");
			if let Err(e) = systemctl_start(config, host, "rollback-watchdog.service")
				.instrument(info_span!("rollback"))
				.await
			{
				error!("failed to trigger rollback: {e}")
			}
		} else {
			info!("trying to mark upgrade as successful");
			let mut cmd = MyCommand::new("rm");
			cmd.arg("-f").arg("/etc/fleet_rollback_marker");
			if let Err(e) = config.run_on(host, cmd, true).in_current_span().await {
				error!("failed to remove rollback marker. This is bad, as the system will be rolled back by watchdog: {e}")
			}
		}
		info!("disarming watchdog, just in case");
		if let Err(_e) = systemctl_stop(config, host, "rollback-watchdog.timer").await {
			// It is ok, if there was no reboot - then timer might not be running.
		}
		if action.should_schedule_rollback_run() {
			if let Err(e) = systemctl_stop(config, host, "rollback-watchdog-run.timer").await {
				error!("failed to disarm rollback run: {e}");
			}
		}
	} else {
		let mut cmd = MyCommand::new("rm");
		cmd.arg("-f").arg("/etc/fleet_rollback_marker");
		if let Err(_e) = config.run_on(host, cmd, true).in_current_span().await {
			// Marker might not exist, yet better try to remove it.
		}
	}
	Ok(())
}

impl BuildSystems {
	async fn build_task(self, config: Config, host: String) -> Result<()> {
		info!("building");
		let action = Action::from(self.subcommand.clone());
		let fleet_field = &config.fleet_field;
		let drv = nix_go!(
			fleet_field.buildSystems(Obj {
				localSystem: { config.local_system.clone() }
			})[{ action.build_attr() }][{ host }]
		);
		let outputs = drv.build().await.map_err(|e| {
			if action.build_attr() == "sdImage" {
				info!("sd-image build failed");
				info!("Make sure you have imported modulesPath/installer/sd-card/sd-image-<arch>[-installer].nix (For installer, you may want to check config)");
			}
			e
		})?;
		let out_output = outputs
			.get("out")
			.ok_or_else(|| anyhow!("system build should produce \"out\" output"))?;

		match action {
			Action::Upload { action } => {
				if !config.is_local(&host) {
					info!("uploading system closure");
					{
						// Alternatively, nix store make-content-addressed can be used,
						// at least for the first deployment, to provide trusted store key.
						//
						// It is much slower, yet doesn't require root on the deployer machine.
						let mut sign = MyCommand::new("nix");
						// Private key for host machine is registered in nix-sign.nix
						sign.arg("store")
							.arg("sign")
							.comparg("--key-file", "/etc/nix/private-key")
							.arg("-r")
							.arg(out_output);
						if let Err(e) = sign.sudo().run_nix().await {
							warn!("Failed to sign store paths: {e}");
						};
					}
					let mut tries = 0;
					loop {
						let mut nix = MyCommand::new("nix");
						nix.arg("copy")
							.arg("--substitute-on-destination")
							.comparg("--to", format!("ssh-ng://{host}"))
							.arg(out_output);
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
					execute_upload(&self, &config, action, &host, out_output.clone()).await?
				}
			}
			Action::Package(PackageAction::SdImage) => {
				let mut out = current_dir()?;
				out.push(format!("sd-image-{}", host));

				info!("linking sd image to {:?}", out);
				symlink(out_output, out)?;
			}
			Action::Package(PackageAction::InstallationCd) => {
				let mut out = current_dir()?;
				out.push(format!("installation-cd-{}", host));

				info!("linking iso image to {:?}", out);
				symlink(out_output, out)?;
			}
		};
		Ok(())
	}

	pub async fn run(self, config: &Config) -> Result<()> {
		let hosts = config.list_hosts().await?;
		let set = LocalSet::new();
		let this = &self;
		for host in hosts.into_iter() {
			if config.should_skip(&host.name) {
				continue;
			}
			let config = config.clone();
			let this = this.clone();
			let span = info_span!("deployment", host = field::display(&host.name));
			let hostname = host.name;
			set.spawn_local(
				(async move {
					match this.build_task(config, hostname).await {
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
