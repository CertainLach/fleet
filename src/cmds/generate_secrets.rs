use std::collections::HashSet;

use anyhow::Result;
use clap::Clap;
use log::info;

use crate::db::{
	keys::KeyDb,
	secret::{list_secrets, SecretDb},
	Db, DbData,
};

#[derive(Clap)]
pub struct GenerateSecrets {
	/// If set - remove orphaned secrets
	#[clap(long)]
	cleanup: bool,
}

impl GenerateSecrets {
	pub fn run(self) -> Result<()> {
		let db = Db::new(".fleet")?;
		let mut secrets = SecretDb::open(&db)?;

		let defined_secrets = list_secrets()?;
		for (secret, data) in defined_secrets.iter() {
			let keys = KeyDb::open(&db)?;
			secrets.ensure_generated(&keys, &secret, &data)?;
		}
		let key_names = defined_secrets
			.keys()
			.filter(|s| !secrets.has_secret(s))
			.cloned()
			.collect::<HashSet<_>>();
		if !key_names.is_empty() {
			if self.cleanup {
				info!("Removed orphan secrets:");
			} else {
				info!("Orphan secrets found, run with --cleanup to remove them from db:");
			}
			for key in key_names {
				info!("- {}", key);
				if self.cleanup {
					secrets.remove_secret(&key)
				}
			}
		}

		Ok(())
	}
}
