use std::io::Write;

use anyhow::Result;
use clap::Clap;

use crate::host::Config;

#[derive(Clap)]
pub enum Secrets {
	/// Force load keys for all defined hosts
	ForceKeys,
	/// Add secret, data should be provided in stdin
	Add {
		/// Secret owner
		machine: String,
		/// Secret name
		name: String,
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
			Secrets::Add { machine, name } => {
				let recipient = config.recipient(&machine)?;
				let encryptor = age::Encryptor::with_recipients(vec![Box::new(recipient)]);

				let mut encrypted = vec![];
				{
					let mut w = encryptor.wrap_output(&mut encrypted)?;

					let stdin = std::io::stdin();
					let mut lock = stdin.lock();
					std::io::copy(&mut lock, &mut w)?;
					w.flush()?;
				}

				config.update_secret(&machine, &name, &encrypted)
			}
		}
		Ok(())
	}
}
