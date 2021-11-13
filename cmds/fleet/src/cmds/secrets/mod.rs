use crate::{
	fleetdata::{FleetSecret, FleetSharedSecret},
	host::Config,
};
use anyhow::{bail, Result};
use std::{
	io::{self, Cursor, Read},
	path::PathBuf,
};
use structopt::StructOpt;

#[derive(StructOpt)]
pub enum Secrets {
	/// Force load keys for all defined hosts
	ForceKeys,
	/// Add secret, data should be provided in stdin
	AddShared {
		/// Secret name
		name: String,
		/// Secret owners
		machines: Vec<String>,
		/// Override secret if already present
		#[structopt(long)]
		force: bool,
		#[structopt(long)]
		public: Option<String>,
		#[structopt(long)]
		public_file: Option<PathBuf>,
	},
	/// Add secret, data should be provided in stdin
	Add {
		/// Secret name
		name: String,
		/// Secret owners
		machine: String,
		/// Override secret if already present
		#[structopt(long)]
		force: bool,
		#[structopt(long)]
		public: Option<String>,
		#[structopt(long)]
		public_file: Option<PathBuf>,
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
			Secrets::AddShared {
				machines,
				name,
				force,
				public,
				public_file,
			} => {
				let recipients = machines
					.iter()
					.map(|m| config.recipient(m))
					.collect::<Result<Vec<_>>>()?;

				let secret = {
					let mut input = vec![];
					io::stdin().read_to_end(&mut input)?;

					if input.is_empty() {
						input
					} else {
						let mut encrypted = vec![];
						let recipients = recipients
							.iter()
							.cloned()
							.map(|r| Box::new(r) as Box<dyn age::Recipient>)
							.collect();
						let mut encryptor = age::Encryptor::with_recipients(recipients)
							.wrap_output(&mut encrypted)?;
						io::copy(&mut Cursor::new(input), &mut encryptor)?;
						encryptor.finish()?;
						encrypted
					}
				};

				let mut data = config.data_mut();
				if data.shared_secrets.contains_key(&name) && !force {
					bail!("secret already defined");
				}
				data.shared_secrets.insert(
					name,
					FleetSharedSecret {
						owners: machines,
						secret: FleetSecret {
							expire_at: None,
							secret,
							public: match (public, public_file) {
								(Some(v), None) => Some(v),
								(None, Some(v)) => Some(std::fs::read_to_string(&v)?),
								(Some(_), Some(_)) => {
									bail!("only public or public_file should be set")
								}
								(None, None) => None,
							},
						},
					},
				);
			}
			Secrets::Add {
				machine,
				name,
				force,
				public,
				public_file,
			} => {
				let recipient = config.recipient(&machine)?;

				let secret = {
					let mut input = vec![];
					io::stdin().read_to_end(&mut input)?;

					let mut encrypted = vec![];
					let recipient = Box::new(recipient) as Box<dyn age::Recipient>;
					let mut encryptor = age::Encryptor::with_recipients(vec![recipient])
						.wrap_output(&mut encrypted)?;
					io::copy(&mut Cursor::new(input), &mut encryptor)?;
					encryptor.finish()?;
					encrypted
				};

				let mut data = config.data_mut();
				let host_secrets = data.host_secrets.entry(machine).or_default();
				if host_secrets.contains_key(&name) && !force {
					bail!("secret already defined");
				}
				host_secrets.insert(
					name,
					FleetSecret {
						expire_at: None,
						secret,
						public: match (public, public_file) {
							(Some(v), None) => Some(v),
							(None, Some(v)) => Some(std::fs::read_to_string(&v)?),
							(Some(_), Some(_)) => bail!("only public or public_file should be set"),
							(None, None) => None,
						},
					},
				);
			}
		}
		Ok(())
	}
}
