use crate::host::FleetOpts;
use anyhow::Result;
use clap::Clap;
use log::{info, warn};

#[derive(Clap)]
pub struct FetchKeys {
	#[clap(flatten)]
	fleet_opts: FleetOpts,

	/// If true - remove orphaned keys
	#[clap(long)]
	cleanup: bool,
}

impl FetchKeys {
	pub fn run(self) -> Result<()> {
		let fleet = self.fleet_opts.build()?;
		let hosts = fleet.list_hosts()?;
		for host in hosts.iter() {
			if host.skip() {
				warn!("Skipped host {}", host.hostname);
				continue;
			}
			host.key()?;
		}
		let orphans: Vec<_> = fleet.list_orphaned_keys()?;
		if !orphans.is_empty() {
			if self.cleanup {
				info!("Removed orphan host keys:");
			} else {
				info!("Orphan host keys found, run with --cleanup to remove them from db:");
			}
			for (name, path) in orphans {
				info!("- {}", name);
				if self.cleanup {
					std::fs::remove_file(path)?;
				}
			}
		}
		Ok(())
	}
}
