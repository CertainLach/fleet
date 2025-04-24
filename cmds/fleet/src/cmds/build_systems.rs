use std::{env::current_dir, os::unix::fs::symlink, path::PathBuf, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, ValueEnum};
use fleet_base::{
	host::{Config, ConfigHost, DeployKind},
	opts::FleetOpts,
};
use itertools::Itertools as _;
use nix_eval::{nix_go, NixBuildBatch};
use tokio::{task::LocalSet, time::sleep};
use tracing::{error, field, info, info_span, warn, Instrument};

#[derive(Parser)]
pub struct Deploy {
	/// Disable automatic rollback
	#[clap(long)]
	disable_rollback: bool,
	/// Action to execute after system is built
	action: DeployAction,
}

#[derive(ValueEnum, Clone, Copy)]
enum DeployAction {
	/// Upload derivation, but do not execute the update.
	Upload,
	/// Upload and execute the activation script, old version will be used after reboot.
	Test,
	/// Upload and set as current system profile, but do not execute activation script.
	Boot,
	/// Upload, set current profile, and execute activation script.
	Switch,
}

impl DeployAction {
	pub(crate) fn name(&self) -> Option<&'static str> {
		match self {
			Self::Upload => None,
			Self::Test => Some("test"),
			Self::Boot => Some("boot"),
			Self::Switch => Some("switch"),
		}
	}
	pub(crate) fn should_switch_profile(&self) -> bool {
		matches!(self, Self::Switch | Self::Boot)
	}
	pub(crate) fn should_activate(&self) -> bool {
		matches!(self, Self::Switch | Self::Test | Self::Boot)
	}
	pub(crate) fn should_create_rollback_marker(&self) -> bool {
		// Upload does nothing on the target machine, other than uploading the closure.
		// In boot case we want to have rollback marker prepared, so that the system may rollback itself on the next boot.
		!matches!(self, Self::Upload)
	}
	pub(crate) fn should_schedule_rollback_run(&self) -> bool {
		matches!(self, Self::Switch | Self::Test)
	}
}

#[derive(Parser, Clone)]
pub struct BuildSystems {
	/// Attribute to build. Systems are deployed from "toplevel" attr, well-known used attributes
	/// are "sdImage"/"isoImage", and your configuration may include any other build attributes.
	#[clap(long, default_value = "toplevel")]
	build_attr: String,
}

struct Generation {
	id: u32,
	current: bool,
	datetime: String,
}

fn parse_generation_line(g: &str) -> Option<Generation> {
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
	Some(Generation {
		id,
		current,
		datetime: format!("{date} {time}"),
	})
}

async fn get_current_generation(host: &ConfigHost) -> Result<Generation> {
	let mut cmd = host.cmd("nix-env").await?;
	cmd.comparg("--profile", "/nix/var/nix/profiles/system")
		.arg("--list-generations");
	// Sudo is required due to --list-generations acquiring lock on the profile.
	let data = cmd.sudo().run_string().await?;
	let generations = data
		.split('\n')
		.map(|e| e.trim())
		.filter(|&l| !l.is_empty())
		.filter_map(|g| {
			let gen = parse_generation_line(g);
			if gen.is_none() {
				warn!("bad generation: {g}");
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

async fn deploy_task(
	action: DeployAction,
	host: &ConfigHost,
	built: PathBuf,
	specialisation: Option<String>,
	disable_rollback: bool,
) -> Result<()> {
	let deploy_kind = host.deploy_kind().await?;
	if (deploy_kind == DeployKind::NixosInstall || deploy_kind == DeployKind::NixosLustrate)
		&& !matches!(action, DeployAction::Boot | DeployAction::Upload)
	{
		bail!("{deploy_kind:?} deploy kind only supports boot and upload actions");
	}

	let mut failed = false;

	// TODO: Lockfile, to prevent concurrent system switch?
	// TODO: If rollback target exists - bail, it should be removed. Lockfile will not work in case if rollback
	// is scheduler on next boot (default behavior). On current boot - rollback activator will fail due to
	// unit name conflict in systemd-run
	// This code is tied to rollback.nix
	if !disable_rollback && action.should_create_rollback_marker() {
		let _span = info_span!("preparing").entered();
		info!("preparing for rollback");
		let generation = get_current_generation(host).await?;
		info!(
			"rollback target would be {} {}",
			generation.id, generation.datetime
		);
		{
			let mut cmd = host.cmd("sh").await?;
			cmd.arg("-c").arg(format!("mark=$(mktemp -p /etc -t fleet_rollback_marker.XXXXX) && echo -n {} > $mark && mv --no-clobber $mark /etc/fleet_rollback_marker", generation.id));
			if let Err(e) = cmd.sudo().run().await {
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
			let mut cmd = host.cmd("systemd-run").await?;
			cmd.comparg("--on-active", "3min")
				.comparg("--unit", "rollback-watchdog-run")
				.arg("systemctl")
				.arg("start")
				.arg("rollback-watchdog.service");
			if let Err(e) = cmd.sudo().run().await {
				error!("failed to schedule rollback run: {e}");
				failed = true;
			}
		}
	}
	if deploy_kind == DeployKind::NixosLustrate {
		// Fleet could also create this file, but as this operation is potentially disruptive,
		// make user do it themself.
		if !host.file_exists("/etc/NIXOS_LUSTRATE").await? {
			bail!("/etc/NIXOS_LUSTRATE should be created on remote host");
		}
		// Wanted by NixOS to recognize the system as NixOS.
		let mut cmd = host.cmd("touch").await?;
		cmd.arg("/etc/NIXOS");
		cmd.sudo().run().await.context("creating /etc/NIXOS")?;
	}
	if deploy_kind == DeployKind::NixosInstall {
		info!(
			"running nixos-install to switch profile, install bootloader, and perform activation"
		);
		let mut cmd = host.cmd("nixos-install").await?;
		cmd.arg("--system").arg(&built).args([
			// Channels here aren't fleet host system channels, but channels embedded in installation cd, which might be old.
			// It is possible to copy host channels, but I would prefer non-flake nix just to be unsupported.
			"--no-channel-copy",
			"--root",
			"/mnt",
		]);
		if let Err(e) = cmd.sudo().run().await {
			error!("failed to execute nixos-install: {e}");
			failed = true;
		}
	} else {
		if action.should_switch_profile() && !failed {
			info!("switching system profile generation");

			// To avoid even more problems, using nixos-install for now.
			// // nix build is unable to work with --store argument for some reason, and nix until 2.26 didn't support copy with --profile argument,
			// // falling back to using nix-env command
			// // After stable NixOS starts using 2.26 - use `nix --store /mnt copy --from /mnt --profile ...` here, and instead of nix build below.
			// let mut cmd = host.cmd("nix-env").await?;
			// cmd.args([
			// 	"--store",
			// 	"/mnt",
			// 	"--profile",
			// 	"/mnt/nix/var/nix/profiles/system",
			// 	"--set",
			// ])
			// .arg(&built);
			// if let Err(e) = cmd.sudo().run_nix().await {
			// 	error!("failed to switch system profile generation: {e}");
			// 	failed = true;
			// }
			// It would also be possible to update profile atomically during copy:
			// https://github.com/NixOS/nix/pull/11657
			let mut cmd = host.nix_cmd().await?;
			cmd.arg("build");
			cmd.comparg("--profile", "/nix/var/nix/profiles/system");
			cmd.arg(&built);
			if let Err(e) = cmd.sudo().run_nix().await {
				error!("failed to switch system profile generation: {e}");
				failed = true;
			}
		}

		// FIXME: Connection might be disconnected after activation run

		if action.should_activate() && !failed {
			let _span = info_span!("activating").entered();
			info!("executing activation script");
			let specialised = if let Some(specialisation) = specialisation {
				let mut specialised = built.join("specialisation");
				specialised.push(specialisation);
				specialised
			} else {
				built.clone()
			};
			let switch_script = specialised.join("bin/switch-to-configuration");
			let mut cmd = host.cmd(switch_script).in_current_span().await?;
			if deploy_kind == DeployKind::NixosLustrate {
				cmd.env("NIXOS_INSTALL_BOOTLOADER", "1");
			}
			cmd.env("FLEET_ONLINE_ACTIVATION", "1")
				.arg(action.name().expect("upload.should_activate == false"));
			if let Err(e) = cmd.sudo().run().in_current_span().await {
				error!("failed to activate: {e}");
				failed = true;
			}
		}
	}
	if action.should_create_rollback_marker() {
		if !disable_rollback {
			if failed {
				if action.should_schedule_rollback_run() {
					info!("executing rollback");
					if let Err(e) = host
						.systemctl_start("rollback-watchdog.service")
						.instrument(info_span!("rollback"))
						.await
					{
						error!("failed to trigger rollback: {e}")
					}
				}
			} else {
				info!("trying to mark upgrade as successful");
				if let Err(e) = host
					.rm_file("/etc/fleet_rollback_marker", true)
					.in_current_span()
					.await
				{
					error!("failed to remove rollback marker. This is bad, as the system will be rolled back by watchdog: {e}")
				}
			}
			info!("disarming watchdog, just in case");
			if let Err(_e) = host.systemctl_stop("rollback-watchdog.timer").await {
				// It is ok, if there was no reboot - then timer might not be running.
			}
			if action.should_schedule_rollback_run() {
				if let Err(e) = host.systemctl_stop("rollback-watchdog-run.timer").await {
					error!("failed to disarm rollback run: {e}");
				}
			}
		} else if let Err(_e) = host
			.rm_file("/etc/fleet_rollback_marker", true)
			.in_current_span()
			.await
		{
			// Marker might not exist, yet better try to remove it.
		}
	}
	Ok(())
}

async fn build_task(
	config: Config,
	hostname: String,
	build_attr: &str,
	batch: Option<NixBuildBatch>,
) -> Result<PathBuf> {
	info!("building");
	let host = config.host(&hostname).await?;
	// let action = Action::from(self.subcommand.clone());
	let nixos = host.nixos_config().await?;
	let drv = nix_go!(nixos.system.build[{ build_attr }]);
	let outputs = drv.build_maybe_batch(batch).await?;
	let out_output = outputs
		.get("out")
		.ok_or_else(|| anyhow!("system build should produce \"out\" output"))?;

	{
		info!("adding gc root");
		let mut cmd = config.local_host().cmd("nix").await?;
		cmd.arg("build")
			.comparg(
				"--profile",
				format!(
					"/nix/var/nix/profiles/{}-{hostname}",
					config.data().gc_root_prefix
				),
			)
			.arg(out_output);
		cmd.sudo().run_nix().await?;
	}

	Ok(out_output.clone())
}

impl BuildSystems {
	pub async fn run(self, config: &Config, opts: &FleetOpts) -> Result<()> {
		let hosts = opts.filter_skipped(config.list_hosts().await?).await?;
		let set = LocalSet::new();
		let build_attr = self.build_attr.clone();
		let batch = (hosts.len() > 1).then(|| {
			config
				.nix_session
				.new_build_batch("build-hosts".to_string())
		});
		for host in hosts {
			let config = config.clone();
			let span = info_span!("build", host = field::display(&host.name));
			let hostname = host.name;
			let build_attr = build_attr.clone();
			let batch = batch.clone();
			set.spawn_local(
				(async move {
					let built = match build_task(config, hostname.clone(), &build_attr, batch).await
					{
						Ok(path) => path,
						Err(e) => {
							error!("failed to deploy host: {}", e);
							return;
						}
					};
					// TODO: Handle error
					let mut out = current_dir().expect("cwd exists");
					out.push(format!("built-{}", hostname));

					info!("linking iso image to {:?}", out);
					if let Err(e) = symlink(built, out) {
						error!("failed to symlink: {e}")
					}
				})
				.instrument(span),
			);
		}
		drop(batch);
		set.await;
		Ok(())
	}
}

impl Deploy {
	pub async fn run(self, config: &Config, opts: &FleetOpts) -> Result<()> {
		let hosts = opts.filter_skipped(config.list_hosts().await?).await?;
		let set = LocalSet::new();
		let batch = (hosts.len() > 1).then(|| {
			config
				.nix_session
				.new_build_batch("deploy-hosts".to_string())
		});
		for host in hosts.into_iter() {
			let config = config.clone();
			let span = info_span!("deploy", host = field::display(&host.name));
			let hostname = host.name.clone();
			let local_host = config.local_host();
			let opts = opts.clone();
			let batch = batch.clone();
			if let Some(deploy_kind) = opts.action_attr::<DeployKind>(&host, "deploy_kind").await? {
				host.set_deploy_kind(deploy_kind);
			};

			set.spawn_local(
				(async move {
					let built =
						match build_task(config.clone(), hostname.clone(), "toplevel", batch).await
						{
							Ok(path) => path,
							Err(e) => {
								error!("failed to build host system closure: {}", e);
								return;
							}
						};

					let deploy_kind = match host.deploy_kind().await {
						Ok(v) => v,
						Err(e) => {
							error!("failed to query target deploy kind: {e}");
							return;
						}
					};

					// TODO: Make disable_rollback a host attribute instead
					let mut disable_rollback = self.disable_rollback;
					if !disable_rollback && deploy_kind != DeployKind::Fleet {
						warn!("disabling rollback, as not supported by non-fleet deployment kinds");
						disable_rollback = true;
					}

					if !opts.is_local(&hostname) {
						info!("uploading system closure");
						{
							// TODO: Move to remote_derivation method.
							// Alternatively, nix store make-content-addressed can be used,
							// at least for the first deployment, to provide trusted store key.
							//
							// It is much slower, yet doesn't require root on the deployer machine.
							let Ok(mut sign) = local_host.cmd("nix").await else {
								error!("failed to setup local");
								return;
							};
							// Private key for host machine is registered in nix-sign.nix
							sign.arg("store")
								.arg("sign")
								.comparg("--key-file", "/etc/nix/private-key")
								.arg("-r")
								.arg(&built);
							if let Err(e) = sign.sudo().run_nix().await {
								warn!("failed to sign store paths: {e}");
							};
						}
						let mut tries = 0;
						loop {
							match host.remote_derivation(&built).await {
								Ok(remote) => {
									assert!(remote == built, "CA derivations aren't implemented");
									break;
								}
								Err(e) if tries < 3 => {
									tries += 1;
									warn!("copy failure ({}/3): {}", tries, e);
									sleep(Duration::from_millis(5000)).await;
								}
								Err(e) => {
									error!("upload failed: {e}");
									return;
								}
							}
						}
					}
					if let Err(e) = deploy_task(
						self.action,
						&host,
						built,
						if let Ok(v) = opts.action_attr(&host, "specialisation").await {
							v
						} else {
							error!("unreachable? failed to get specialization");
							return;
						},
						disable_rollback,
					)
					.await
					{
						error!("activation failed: {e}");
					}
				})
				.instrument(span),
			);
		}
		drop(batch);
		set.await;
		Ok(())
	}
}
