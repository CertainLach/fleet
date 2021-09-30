use crate::{fleetdata::FleetSecret, host::Config};
use anyhow::{bail, Result};
use std::io::{self, Cursor, Read};
use structopt::StructOpt;

#[derive(StructOpt)]
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
		#[structopt(long)]
		force: bool,
		#[structopt(long)]
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
					encryptor.finish()?;
					encrypted
				};

				let mut data = config.data_mut();
				if data.secrets.contains_key(&name) && !force {
					bail!("secret already defined");
				}
				data.secrets.insert(
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
