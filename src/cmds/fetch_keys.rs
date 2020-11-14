use crate::db::{
	keys::{list_hosts, KeyDb},
	Db, DbData,
};
use anyhow::Result;
use clap::Clap;
use log::info;

#[derive(Clap)]
pub struct FetchKeys {
	/// Fetch if already exists the following hosts
	#[clap(short = 'f', long)]
	force_hosts: Vec<String>,
	/// If true - remove orphaned keys
	#[clap(long)]
	cleanup: bool,
}

impl FetchKeys {
	pub fn run(self) -> Result<()> {
		let db = Db::new(".fleet")?;
		let hosts = list_hosts()?;
		let mut keys = KeyDb::open(&db)?;
		for host in hosts.iter() {
			let force = self.force_hosts.contains(&host);
			keys.ensure_key_loaded(host, force)?;
		}
		let orphans: Vec<_> = hosts.iter().filter(|h| !keys.has_key(h)).cloned().collect();
		if !orphans.is_empty() {
			if self.cleanup {
				info!("Removed orphan host keys:");
			} else {
				info!("Orphan host keys found, run with --cleanup to remove them from db:");
			}
			for key in orphans {
				info!("- {}", key);
				if self.cleanup {
					keys.remove_key(&key)
				}
			}
		}
		Ok(())
	}
}
