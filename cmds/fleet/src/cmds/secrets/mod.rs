use crate::{
	fleetdata::{FleetSecret, FleetSharedSecret},
	host::Config,
};
use age::{Decryptor, Encryptor};
use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use futures::{StreamExt, TryStreamExt};
use std::{
	collections::HashSet,
	io::{self, Cursor, Read, Write},
	iter,
	path::PathBuf,
};
use tracing::{info, warn};

#[derive(Parser)]
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
		#[clap(long)]
		force: bool,
		#[clap(long)]
		public: Option<String>,
		#[clap(long)]
		public_file: Option<PathBuf>,
	},
	/// Add secret, data should be provided in stdin
	Add {
		/// Secret name
		name: String,
		/// Secret owners
		machine: String,
		/// Override secret if already present
		#[clap(long)]
		force: bool,
		#[clap(long)]
		public: Option<String>,
		#[clap(long)]
		public_file: Option<PathBuf>,
	},
	/// Read secret from remote host, requires sudo on said host
	Read {
		name: String,
		machine: String,
	},
	UpdateShared {
		name: String,

		machines: Option<Vec<String>>,

		add_machines: Vec<String>,
		remove_machines: Vec<String>,

		/// Which host should we use to decrypt
		prefer_identities: Vec<String>,
	},
	Regenerate,
}

impl Secrets {
	pub async fn run(self, config: &Config) -> Result<()> {
		match self {
			Secrets::ForceKeys => {
				for host in config.list_hosts().await? {
					if config.should_skip(&host) {
						continue;
					}
					config.key(&host).await?;
				}
			}
			Secrets::AddShared {
				machines,
				name,
				force,
				public,
				public_file,
			} => {
				let recipients = futures::stream::iter(machines.iter())
					.then(|m| config.recipient(m))
					.try_collect::<Vec<_>>()
					.await?;

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
							.map(|r| Box::new(r) as Box<dyn age::Recipient + Send>)
							.collect();
						let mut encryptor = age::Encryptor::with_recipients(recipients)
							.expect("recipients provided")
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
								(None, Some(v)) => Some(std::fs::read_to_string(v)?),
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
				let recipient = config.recipient(&machine).await?;

				let secret = {
					let mut input = vec![];
					io::stdin().read_to_end(&mut input)?;
					if input.is_empty() {
						bail!("no data provided")
					}

					let mut encrypted = vec![];
					let recipient = Box::new(recipient) as Box<dyn age::Recipient + Send>;
					let mut encryptor = age::Encryptor::with_recipients(vec![recipient])
						.expect("recipients provided")
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
							(None, Some(v)) => Some(std::fs::read_to_string(v)?),
							(Some(_), Some(_)) => bail!("only public or public_file should be set"),
							(None, None) => None,
						},
					},
				);
			}
			// TODO: Instead of using sudo, decode secret on remote machine
			#[allow(clippy::await_holding_refcell_ref)]
			Secrets::Read { name, machine } => {
				let data = config.data();

				let Some(host_secrets) = data.host_secrets.get(&machine) else {
                    bail!("no secrets for machine {machine}");
                };
				let Some(secret) = host_secrets.get(&name) else {
                    bail!("machine {machine} has no secret {name}");
                };
				if secret.secret.is_empty() {
					bail!("no secret {name}");
				}
				let identity = config.identity(&machine).await?;
				let decryptor = Decryptor::new(Cursor::new(&secret.secret))?;
				let decryptor = match decryptor {
					Decryptor::Recipients(r) => r,
					Decryptor::Passphrase(_) => bail!("should be recipients"),
				};
				let mut decryptor = decryptor
					.decrypt(iter::once(&identity as &dyn age::Identity))
					.context("failed to decrypt, wrong key?")?;

				let mut decrypted = Vec::new();
				decryptor
					.read_to_end(&mut decrypted)
					.context("failed to decrypt")?;
				// secret.secret
				std::io::stdout().lock().write_all(&decrypted)?;
			}
			Secrets::UpdateShared {
				name,
				machines,
				mut add_machines,
				mut remove_machines,
				prefer_identities,
			} => {
				let mut data = config.data_mut();
				if machines.is_none() && add_machines.is_empty() && remove_machines.is_empty() {
					bail!("no operation");
				}

				let Some(mut secret) = data.shared_secrets.get_mut(&name) else {
                    bail!("no shared secret {name}");
                };
				if secret.secret.secret.is_empty() {
					bail!("no secret");
				}

				let initial_machines = secret.owners.clone();
				let mut target_machines = secret.owners.clone();

				// ensure!(machines.is_some() || !add_machines.is_empty() || )
				if let Some(machines) = machines {
					ensure!(
						add_machines.is_empty() && remove_machines.is_empty(),
						"can't combine --machines and --add-machines/--remove-machines"
					);
					let target = initial_machines.iter().collect::<HashSet<_>>();
					let source = machines.iter().collect::<HashSet<_>>();
					for removed in target.difference(&source) {
						remove_machines.push((*removed).clone());
					}
					for added in source.difference(&target) {
						add_machines.push((*added).clone());
					}
				}

				for machine in &remove_machines {
					let mut removed = false;
					while let Some(pos) = target_machines.iter().position(|m| m == machine) {
						target_machines.swap_remove(pos);
						removed = true;
					}
					if !removed {
						bail!("secret is not enabled for {machine}");
					}
				}
				for machine in &add_machines {
					if target_machines.iter().any(|m| m == machine) {
						warn!("secret is already added to {machine}");
					}
				}
				if remove_machines.is_empty() {
					warn!("secret will not be regenerated for removed machines, and until host rebuild, they will still possess the ability to decode secret");
				}
				if target_machines.is_empty() {
					info!("no machines left for secret, removing it");
					data.shared_secrets.remove(&name);
					return Ok(());
				}

				let identity_holder = if !prefer_identities.is_empty() {
					prefer_identities
						.iter()
						.find(|i| initial_machines.iter().any(|s| s == *i))
				} else {
					secret.owners.first()
				};
				let Some(identity_holder) = identity_holder else {
                    bail!("no available holder found");
                };
				let target_recipients = futures::stream::iter(&target_machines)
					.flat_map(|m| futures::stream::once(config.recipient(m)))
					.collect::<Vec<_>>()
					.await
					.into_iter()
					.map(|v| v.map(|v| Box::new(v) as Box<dyn age::Recipient + Send>))
					.collect::<Result<Vec<_>>>()?;

				let identity = config.identity(identity_holder).await?;
				let decryptor = Decryptor::new(Cursor::new(&secret.secret.secret))?;
				let decryptor = match decryptor {
					Decryptor::Recipients(r) => r,
					Decryptor::Passphrase(_) => bail!("should be recipients"),
				};
				let mut decryptor = decryptor
					.decrypt(iter::once(&identity as &dyn age::Identity))
					.context("failed to decrypt, wrong key?")?;

				let mut decrypted = Vec::new();
				decryptor
					.read_to_end(&mut decrypted)
					.context("failed to decrypt")?;

				let mut encrypted = vec![];
				let mut encryptor = Encryptor::with_recipients(target_recipients)
					.expect("recipients provided")
					.wrap_output(&mut encrypted)?;
				io::copy(&mut Cursor::new(decrypted), &mut encryptor)?;
				encryptor.finish()?;

				secret.secret.secret = encrypted;
			}
			Secrets::Regenerate => {
				// config.data_mut().shared_secrets
				{
					let expected_shared_set =
						config.shared_config_attr_names("sharedSecrets").await?;
					let expected_shared_set = expected_shared_set.iter().collect::<HashSet<_>>();
					let shared_set = config.data();
					let shared_set = shared_set.shared_secrets.keys().collect::<HashSet<_>>();
					for removed in expected_shared_set.difference(&shared_set) {
						warn!("secret needs to be generated: {removed}")
					}
				}
				let mut to_remove = Vec::new();
				for (name, data) in &config.data().shared_secrets {
					let expected_owners: Vec<String> = config
						.shared_config_attr(&format!("sharedSecrets.\"{name}\".expectedOwners"))
						.await?;
					if expected_owners.is_empty() {
						warn!("secret was removed from fleet config: {name}, removing from data");
						to_remove.push(name.to_string());
						continue;
					}
					let set = data.owners.iter().collect::<HashSet<_>>();
					let expected_set = expected_owners.iter().collect::<HashSet<_>>();
					if set != expected_set {
						warn!("reconfiguring owners for {name}");
					}
				}
				for k in to_remove {
					config.data_mut().shared_secrets.remove(&k);
				}
			}
		}
		Ok(())
	}
}
