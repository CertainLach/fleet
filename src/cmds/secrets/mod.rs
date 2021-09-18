use crate::{fleetdata::FleetSecret, host::Config};
use anyhow::{bail, Result};
use clap::Clap;
use std::io::{self, Cursor, Read};

#[derive(Clap)]
pub enum Secrets {
	/// Force load keys for all defined hosts
	ForceKeys,
	/// Add secret, data should be provided in stdin
	Add {
		/// Secret name
		name: String,
		/// Secret owners
		machines: Vec<String>,
		/// Override secret if already present
		#[clap(long)]
		force: bool,
		#[clap(long)]
		public: Option<String>,
	},
}

impl Secrets {
	pub fn run(self, config: &Config) -> Result<()> {
		match self {
			Secrets::ForceKeys => {
				for host in config.list_hosts()? {
					if config.should_skip(&host) {
						continue;
					}
					config.key(&host)?;
				}
			}
			Secrets::Add {
				machines,
				name,
				force,
				public,
			} => {
				let recipients = machines
					.iter()
					.map(|m| config.recipient(m))
					.collect::<Result<Vec<_>>>()?;

				let secret = {
					let mut input = vec![];
					io::stdin().read_to_end(&mut input)?;

					let mut encrypted = vec![];
					let recipients = recipients
						.iter()
						.cloned()
						.map(|r| Box::new(r) as Box<dyn age::Recipient>)
						.collect();
					let mut encryptor =
						age::Encryptor::with_recipients(recipients).wrap_output(&mut encrypted)?;
					io::copy(&mut Cursor::new(input), &mut encryptor)?;
					ascii85::encode(&encrypted)
				};

				let mut data = config.data_mut();
				if data.secret.contains_key(&name) && !force {
					bail!("secret already defined");
				}
				data.secret.insert(
					name,
					FleetSecret {
						owners: machines,
						expire_at: None,
						secret,
						public,
					},
				);
			}
		}
		Ok(())
	}
}
