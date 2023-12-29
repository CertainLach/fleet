use crate::{
	better_nix_eval::Field,
	fleetdata::{FleetSecret, FleetSharedSecret, SecretData},
	host::Config,
	nix_go, nix_go_json,
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use futures::StreamExt;
use itertools::Itertools;
use owo_colors::OwoColorize;
use std::{
	collections::HashSet,
	io::{self, Cursor, Read},
	path::PathBuf,
};
use tabled::{Table, Tabled};
use tokio::fs::read_to_string;
use tracing::{info, info_span, warn};

#[derive(Parser)]
pub enum Secret {
	/// Force load host keys for all defined hosts
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
		/// Secret public part
		#[clap(long)]
		public: Option<String>,
		/// Load public part from specified file
		#[clap(long)]
		public_file: Option<PathBuf>,

		/// Create a notification on secret expiration
		#[clap(long)]
		expires_at: Option<DateTime<Utc>>,

		/// Secret with this name already exists, override its value while keeping the same owners.
		#[clap(long)]
		re_add: bool,
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
	List {},
}

async fn generate_shared(
	config: &Config,
	display_name: &str,
	secret: Field,
) -> Result<FleetSharedSecret> {
	Ok(if secret.has_field("generateImpure").await? {
		let config_field = &config.config_unchecked_field;
		let generate = nix_go!(secret.generateImpure);
		let owners: Vec<String> = nix_go_json!(secret.expectedOwners);

		let on: String = nix_go_json!(generate.on);
		let call_package = nix_go!(
			config_field.buildableSystems(Obj {
				localSystem: { config.local_system.clone() }
			})[{ on }]
			.config
			.nixpkgs
			.resolvedPkgs
			.callPackage
		);

		let host = config.host(&on).await?;

		let generator = nix_go!(call_package(generate.generator)(Obj {}));
		let generator = generator.build().await?;
		let generator = generator
			.get("out")
			.ok_or_else(|| anyhow!("missing generateImpure out"))?;
		let generator = host.remote_derivation(generator).await?;

		let mut recipients = String::new();
		for owner in &owners {
			let key = config.key(owner).await?;
			recipients.push_str(&format!("-r \"{key}\" "));
		}
		recipients.push_str("-e");

		let out = host.mktemp_dir().await?;

		let mut gen = host.cmd(generator).await?;
		gen.env("rageArgs", recipients).env("out", &out);
		gen.run().await?;

		{
			let marker = host.read_file_text(format!("{out}/marker")).await?;
			ensure!(marker == "SUCCESS", "generation not succeeded");
		}

		let public = host.read_file_text(format!("{out}/public")).await.ok();
		let secret = host.read_file_bin(format!("{out}/secret")).await.ok();
		if let Some(secret) = &secret {
			ensure!(
				age::Decryptor::new(Cursor::new(&secret)).is_ok(),
				"builder produced non-encrypted value as secret, this is highly insecure"
			);
		}

		let created_at = host.read_file_value(format!("{out}/created_at")).await?;
		let expires_at = host.read_file_value(format!("{out}/expires_at")).await.ok();

		FleetSharedSecret {
			owners,
			secret: FleetSecret {
				created_at,
				expires_at,
				public,
				secret: secret.map(SecretData),
			},
		}
	} else {
		bail!("no generator defined for {display_name}")
	})
}

async fn parse_public(
	public: Option<String>,
	public_file: Option<PathBuf>,
) -> Result<Option<String>> {
	Ok(match (public, public_file) {
		(Some(v), None) => Some(v),
		(None, Some(v)) => Some(read_to_string(v).await?),
		(Some(_), Some(_)) => {
			bail!("only public or public_file should be set")
		}
		(None, None) => None,
	})
}

fn parse_machines(
	initial: Vec<String>,
	machines: Option<Vec<String>>,
	mut add_machines: Vec<String>,
	mut remove_machines: Vec<String>,
) -> Result<Vec<String>> {
	if machines.is_none() && add_machines.is_empty() && remove_machines.is_empty() {
		bail!("no operation");
	}

	let initial_machines = initial.clone();
	let mut target_machines = initial;
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
		// TODO: maybe force secret regeneration?
		// Not that useful without revokation.
		warn!("secret will not be regenerated for removed machines, and until host rebuild, they will still possess the ability to decode secret");
	}
	Ok(target_machines)
}
impl Secret {
	pub async fn run(self, config: &Config) -> Result<()> {
		match self {
			Secret::ForceKeys => {
				for host in config.list_hosts().await? {
					if config.should_skip(&host.name) {
						continue;
					}
					config.key(&host.name).await?;
				}
			}
			Secret::AddShared {
				mut machines,
				name,
				force,
				public,
				public_file,
				expires_at,
				re_add,
			} => {
				let exists = config.has_shared(&name);
				if exists && !force && !re_add {
					bail!("secret already defined");
				}
				if re_add {
					// Fixme: use clap to limit this usage
					ensure!(!force, "--force and --readd are not compatible");
					ensure!(exists, "secret doesn't exists");
					ensure!(
						machines.is_empty(),
						"you can't use machines argument for --readd"
					);
					let shared = config.shared_secret(&name)?;
					machines = shared.owners;
				}

				let recipients = config
					.recipients(&machines.iter().map(String::as_str).collect_vec())
					.await?;

				let secret = {
					let mut input = vec![];
					io::stdin().read_to_end(&mut input)?;

					if input.is_empty() {
						None
					} else {
						Some(
							SecretData::encrypt(recipients, input)
								.ok_or_else(|| anyhow!("no recipients provided"))?,
						)
					}
				};
				let public = parse_public(public, public_file).await?;
				config.replace_shared(
					name,
					FleetSharedSecret {
						owners: machines,
						secret: FleetSecret {
							created_at: Utc::now(),
							expires_at,
							secret,
							public,
						},
					},
				);
			}
			Secret::Add {
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

					Some(SecretData::encrypt(vec![recipient], input).expect("recipient provided"))
				};

				if config.has_secret(&machine, &name) && !force {
					bail!("secret already defined");
				}
				let public = parse_public(public, public_file).await?;

				config.insert_secret(
					&machine,
					name,
					FleetSecret {
						created_at: Utc::now(),
						expires_at: None,
						secret,
						public,
					},
				);
			}
			#[allow(clippy::await_holding_refcell_ref)]
			Secret::Read {
				name,
				machine,
				plaintext,
			} => {
				let secret = config.host_secret(&machine, &name)?;
				let Some(secret) = secret.secret else {
					bail!("no secret {name}");
				};
				let host = config.host(&machine).await?;
				let data = host.decrypt(secret).await?;
				if plaintext {
					let s = String::from_utf8(data).context("output is not utf8")?;
					print!("{s}");
				} else {
					println!("{}", z85::encode(&data));
				}
			}
			Secret::UpdateShared {
				name,
				machines,
				add_machines,
				remove_machines,
				prefer_identities,
			} => {
				let mut secret = config.shared_secret(&name)?;
				if secret.secret.secret.is_none() {
					bail!("no secret");
				}

				let initial_machines = secret.owners.clone();
				let target_machines = parse_machines(
					initial_machines.clone(),
					machines,
					add_machines,
					remove_machines,
				)?;

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

				if let Some(data) = secret.secret.secret {
					let host = config.host(&identity_holder).await?;
					let encrypted = host.reencrypt(data, target_recipients).await?;
					secret.secret.secret = Some(encrypted);
				}

				secret.owners = target_machines;
				config.replace_shared(name, secret);
			}
			Secret::Regenerate { prefer_identities } => {
				{
					let expected_shared_set = config
						.list_configured_shared()
						.await?
						.into_iter()
						.collect::<HashSet<_>>();
					let shared_set = config.list_shared().into_iter().collect::<HashSet<_>>();
					for removed in expected_shared_set.difference(&shared_set) {
						info!("generating secret: {removed}");
						let config_field = &config.config_unchecked_field;
						let config_field = nix_go!(config_field.configUnchecked);
						let secret = nix_go!(config_field.sharedSecrets[{ removed }]);
						let shared = generate_shared(config, removed, secret).await?;
						config.replace_shared(removed.to_string(), shared)
					}
				}
				let mut to_remove = Vec::new();
				for name in &config.list_shared() {
					info!("updating secret: {name}");
					let mut data = config.shared_secret(name)?;
					let config_field = &config.config_unchecked_field;
					let config_field = nix_go!(config_field.configUnchecked);
					let expected_owners: Vec<String> =
						nix_go_json!(config_field.sharedSecrets[{ name }].expectedOwners);
					if expected_owners.is_empty() {
						warn!("secret was removed from fleet config: {name}, removing from data");
						to_remove.push(name.to_string());
						continue;
					}
					let set = data.owners.iter().collect::<HashSet<_>>();
					let expected_set = expected_owners.iter().collect::<HashSet<_>>();
					let should_remove = set.difference(&expected_set).next().is_some();
					if set == expected_set {
						info!("secret data is ok");
						continue;
					}

					let secret = nix_go!(config_field.sharedSecrets[{ name }]);
					let owner_dependent: bool = nix_go_json!(secret.ownerDependent);
					let regenerate_on_remove: bool = nix_go_json!(secret.regenerateOnOwnerRemoved);
					#[allow(clippy::nonminimal_bool)]
					if !owner_dependent && !(should_remove && regenerate_on_remove) {
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

						if let Some(secret) = data.secret.secret {
							let host = config.host(identity_holder).await?;
							let encrypted = host.reencrypt(secret, target_recipients).await?;

							data.secret.secret = Some(encrypted);
						}
						data.owners = expected_owners;
						config.replace_shared(name.to_owned(), data);
					} else {
						let shared = generate_shared(config, name, secret).await?;
						config.replace_shared(name.to_owned(), shared)
					}
				}
				for k in to_remove {
					config.remove_shared(&k);
				}
			}
			Secret::List {} => {
				let _span = info_span!("loading secrets").entered();
				let configured = config.list_configured_shared().await?;
				#[derive(Tabled)]
				struct SecretDisplay {
					#[tabled(rename = "Name")]
					name: String,
					#[tabled(rename = "Owners")]
					owners: String,
				}
				let mut table = vec![];
				for name in configured.iter().cloned() {
					let config = config.clone();
					let expected_owners = config.shared_secret_expected_owners(&name).await?;
					let data = config.shared_secret(&name)?;
					let owners = data
						.owners
						.iter()
						.map(|o| {
							if expected_owners.contains(o) {
								o.green().to_string()
							} else {
								o.red().to_string()
							}
						})
						.collect::<Vec<_>>();
					table.push(SecretDisplay {
						owners: owners.join(", "),
						name,
					})
				}
				info!("loaded\n{}", Table::new(table).to_string())
			}
		}
		Ok(())
	}
}
