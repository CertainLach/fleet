use crate::{fleetdata::FleetSecret, host::Config};
use anyhow::{bail, Result};
use clap::Clap;
use std::{
	collections::BTreeMap,
	io::{Cursor, Read},
};

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
			} => {
				let recipients = machines
					.iter()
					.map(|m| config.recipient(&m))
					.collect::<Result<Vec<_>>>()?;

				let secret_data = {
					let mut input = vec![];
					std::io::stdin().read_to_end(&mut input)?;

					let data: BTreeMap<String, String> = serde_json::from_slice(&input)?;
					let mut transformed_data: BTreeMap<String, String> = BTreeMap::new();
					for (k, v) in data {
						if k.ends_with("_pub") {
							transformed_data.insert(k, v);
						} else if k.ends_with("_secret") {
							let mut encrypted = vec![];
							let recipients = recipients
								.iter()
								.cloned()
								.map(|r| Box::new(r) as Box<dyn age::Recipient>)
								.collect();
							let mut encryptor = age::Encryptor::with_recipients(recipients)
								.wrap_output(&mut encrypted)?;
							std::io::copy(&mut Cursor::new(v.as_bytes()), &mut encryptor)?;
							drop(encryptor);

							transformed_data.insert(k, ascii85::encode(&encrypted));
						} else {
							bail!("unknown key type: {:?}", k);
						}
					}
					transformed_data
				};

				let mut data = config.data_mut();
				if data.secrets.contains_key(&name) && !force {
					bail!("secret already defined");
				}
				data.secrets.insert(
					name,
					FleetSecret {
						owners: machines.clone(),
						expire_at: None,
						data: secret_data,
					},
				);
			}
		}
		Ok(())
	}
}
