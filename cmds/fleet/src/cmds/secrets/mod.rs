use crate::{
	fleetdata::{FleetSecret, FleetSharedSecret},
	host::Config,
};
use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use futures::{StreamExt, TryStreamExt};
use std::{
	collections::HashSet,
	io::{self, Cursor, Read},
	path::PathBuf,
};
use tokio::fs::read_to_string;
use tracing::{info, warn, error};

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
		#[clap(long)]
		plaintext: bool,
	},
	UpdateShared {
		name: String,

		#[clap(long)]
		machines: Option<Vec<String>>,

		#[clap(long)]
		add_machines: Vec<String>,
		#[clap(long)]
		remove_machines: Vec<String>,

		/// Which host should we use to decrypt
		#[clap(long)]
		prefer_identities: Vec<String>,
	},
	Regenerate {
		/// Which host should we use to decrypt, in case if reencryption is required, without
		/// regeneration
		#[clap(long)]
		prefer_identities: Vec<String>,
	},
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

				if config.has_shared(&name) && !force {
					bail!("secret already defined");
				}
				config.replace_shared(
					name,
					FleetSharedSecret {
						owners: machines,
						secret: FleetSecret {
							expire_at: None,
							secret,
							public: match (public, public_file) {
								(Some(v), None) => Some(v),
								(None, Some(v)) => Some(read_to_string(v).await?),
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

				if config.has_secret(&machine, &name) && !force {
					bail!("secret already defined");
				}
				config.insert_secret(
					&machine,
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
			Secrets::Read {
				name,
				machine,
				plaintext,
			} => {
				let secret = config.host_secret(&machine, &name)?;
				if secret.secret.is_empty() {
					bail!("no secret {name}");
				}
				let data = config.decrypt_on_host(&machine, secret.secret).await?;
				if plaintext {
					let s = String::from_utf8(data).context("output is not utf8")?;
					print!("{s}");
				} else {
					println!("{}", z85::encode(&data));
				}
			}
			Secrets::UpdateShared {
				name,
				machines,
				mut add_machines,
				mut remove_machines,
				prefer_identities,
			} => {
				if machines.is_none() && add_machines.is_empty() && remove_machines.is_empty() {
					bail!("no operation");
				}

				let mut secret = config.shared_secret(&name)?;
				if secret.secret.secret.is_empty() {
					bail!("no secret");
				}

				let initial_machines = secret.owners.clone();
				let mut target_machines = secret.owners.clone();
				info!("Currently encrypted for {initial_machines:?}");

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
						warn!("secret is not enabled for {machine}");
					}
				}
				for machine in &add_machines {
					if target_machines.iter().any(|m| m == machine) {
						warn!("secret is already added to {machine}");
					} else {
						target_machines.push(machine.to_owned());
					}
				}
				if !remove_machines.is_empty() {
					warn!("secret will not be regenerated for removed machines, and until host rebuild, they will still possess the ability to decode secret");
				}

				if target_machines.is_empty() {
					info!("no machines left for secret, removing it");
					config.remove_shared(&name);
					return Ok(());
				}

				if target_machines == initial_machines {
					warn!("secret owners are already correct");
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
					.then(|m| async { config.key(m).await })
					.collect::<Vec<_>>()
					.await;
				let target_recipients =
					target_recipients.into_iter().collect::<Result<Vec<_>>>()?;

				let encrypted = config
					.reencrypt_on_host(&identity_holder, secret.secret.secret, target_recipients)
					.await?;

				secret.owners = target_machines;
				secret.secret.secret = encrypted;
				config.replace_shared(name, secret);
			}
			Secrets::Regenerate { prefer_identities } => {
				{
					let expected_shared_set =
						config.shared_config_attr_names("sharedSecrets").await?;
					let expected_shared_set = expected_shared_set.iter().collect::<HashSet<_>>();
					let shared_set = config.list_shared();
					let shared_set = shared_set.iter().collect::<HashSet<_>>();
					for removed in expected_shared_set.difference(&shared_set) {
						warn!("secret needs to be generated: {removed}")
					}
				}
				let mut to_remove = Vec::new();
				for name in &config.list_shared() {
					info!("updating secret: {name}");
					let mut data = config.shared_secret(name)?;
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
					let should_remove = set.difference(&expected_set).next().is_some();
					if set != expected_set {
						let owner_dependent: bool = config
							.shared_config_attr(&format!("sharedSecrets.\"{name}\".ownerDependent"))
							.await?;
						if !owner_dependent {
							warn!("reencrypting secret '{name}' for new owner set");
							// TODO: force regeneration
							if should_remove {
								warn!("secret will not be regenerated for removed machines, and until host rebuild, they will still possess the ability to decode secret");
							}

							let identity_holder = if !prefer_identities.is_empty() {
								prefer_identities
									.iter()
									.find(|i| data.owners.iter().any(|s| s == *i))
							} else {
								data.owners.first()
							};
							let Some(identity_holder) = identity_holder else {
								bail!("no available holder found");
							};

							let target_recipients = futures::stream::iter(&expected_owners)
								.then(|m| async { config.key(m).await })
								.collect::<Vec<_>>()
								.await;
							let target_recipients =
								target_recipients.into_iter().collect::<Result<Vec<_>>>()?;

							let encrypted = config
								.reencrypt_on_host(
									&identity_holder,
									data.secret.secret,
									target_recipients,
								)
								.await?;

							data.secret.secret = encrypted;
							data.owners = expected_owners;
							config.replace_shared(name.to_owned(), data);
						} else if let Some(generator) = config
							.shared_config_attr::<Option<String>>(&format!("sharedSecrets.\"{name}\".generator"))
							.await?
						{
							todo!("regenerate secret {name} with {generator}");
						} else {
							error!("secret '{name}' should be regenerated manually");
						}
					} else {
						info!("secret data is ok")
					}
				}
				for k in to_remove {
					config.remove_shared(&k);
				}
			}
		}
		Ok(())
	}
}
