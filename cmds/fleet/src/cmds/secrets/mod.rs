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
use serde::Deserialize;
use std::{
	collections::{BTreeSet, HashSet},
	io::{self, Cursor, Read},
	path::PathBuf,
};
use tabled::{Table, Tabled};
use tokio::fs::read_to_string;
use tracing::{error, info, info_span, warn, Instrument};

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

#[tracing::instrument(skip(config, secret, field, prefer_identities))]
async fn update_owner_set(
	secret_name: &str,
	config: &Config,
	mut secret: FleetSharedSecret,
	field: Field,
	updated_set: &[String],
	prefer_identities: &[String],
) -> Result<FleetSharedSecret> {
	let original_set = secret.owners.clone();

	let set = original_set.iter().collect::<BTreeSet<_>>();
	let expected_set = updated_set.iter().collect::<BTreeSet<_>>();

	if set == expected_set {
		info!("no need to update owner list, it is already correct");
		return Ok(secret);
	}

	let should_regenerate = if set.difference(&expected_set).next().is_some() {
		// TODO: Remove this warning for revokable secrets.
		warn!("host was removed from secret owners, but until this host rebuild, the secret will still be stored on it.");
		nix_go_json!(field.regenerateOnOwnerRemoved)
	} else if expected_set.difference(&set).next().is_some() {
		nix_go_json!(field.regenerateOnOwnerAdded)
	} else {
		false
	};

	if should_regenerate {
		info!("secret is owner-dependent, will regenerate");
		let generated = generate_shared(config, secret_name, field, updated_set.to_vec()).await?;
		Ok(generated)
	} else {
		let identity_holder = if !prefer_identities.is_empty() {
			prefer_identities
				.iter()
				.find(|i| original_set.iter().any(|s| s == *i))
		} else {
			secret.owners.first()
		};
		let Some(identity_holder) = identity_holder else {
			bail!("no available holder found");
		};

		if let Some(data) = secret.secret.secret {
			let host = config.host(identity_holder).await?;
			let encrypted = host.reencrypt(data, updated_set.to_vec()).await?;
			secret.secret.secret = Some(encrypted);
		}

		secret.owners = updated_set.to_vec();
		Ok(secret)
	}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum GeneratorKind {
	Impure,
}

async fn generate_impure(
	config: &Config,
	_display_name: &str,
	secret: Field,
	default_generator: Field,
	owners: &[String],
) -> Result<FleetSecret> {
	let config_field = &config.config_unchecked_field;
	let generator = nix_go!(secret.generator);

	let on: String = nix_go_json!(default_generator.impureOn);
	let call_package = nix_go!(
		config_field.hosts[{ on }]
			.nixosSystem
			.config
			.nixpkgs
			.resolvedPkgs
			.callPackage
	);

	let host = config.host(&on).await?;

	let generator = nix_go!(call_package(generator)(Obj {}));
	let generator = generator.build().await?;
	let generator = generator
		.get("out")
		.ok_or_else(|| anyhow!("missing generateImpure out"))?;
	let generator = host.remote_derivation(generator).await?;

	let mut recipients = String::new();
	for owner in owners {
		let key = config.key(owner).await?;
		recipients.push_str(&format!("-r \"{key}\" "));
	}
	recipients.push_str("-e");

	let out = host.mktemp_dir().await?;

	let mut gen = host.cmd(generator).await?;
	gen.env("rageArgs", recipients).env("out", &out);
	gen.run().await.context("impure generator")?;

	{
		let marker = host.read_file_text(format!("{out}/marker")).await?;
		ensure!(marker == "SUCCESS", "generation not succeeded");
	}

	let public = host.read_file_text(format!("{out}/public")).await.ok();
	let secret = host.read_file_bin(format!("{out}/secret")).await.ok();
	if let Some(secret) = &secret {
		ensure!(
			age::Decryptor::new(Cursor::new(&secret)).is_ok(),
			"builder produced non-encrypted value as secret, this is highly insecure, and not allowed."
		);
	}

	let created_at = host.read_file_value(format!("{out}/created_at")).await?;
	let expires_at = host.read_file_value(format!("{out}/expires_at")).await.ok();

	Ok(FleetSecret {
		created_at,
		expires_at,
		public,
		secret: secret.map(SecretData),
	})
}
async fn generate(
	config: &Config,
	display_name: &str,
	secret: Field,
	owners: &[String],
) -> Result<FleetSecret> {
	let generator = nix_go!(secret.generator);
	// Can't properly check on nix module system level
	{
		let gen_ty = generator.type_of().await?;
		if gen_ty == "null" {
			bail!("secret has no generator defined, can't automatically generate it.");
		}
		if gen_ty != "lambda" {
			bail!("generator should be lambda, got {gen_ty}");
		}
	}
	let default_pkgs = &config.default_pkgs;
	let default_call_package = nix_go!(default_pkgs.callPackage);
	// Generators provide additional information in passthru, to access
	// passthru we should call generator, but information about where this generator is supposed to build
	// is located in passthru... Thus evaluating generator on host.
	//
	// Maybe it is also possible to do some magic with __functor?
	//
	// I don't want to make modules always responsible for additional secret data anyway,
	// so it should be in derivation, and not in the secret data itself.
	let default_generator = nix_go!(default_call_package(generator)(Obj {}));

	let kind: GeneratorKind = nix_go_json!(default_generator.generatorKind);

	match kind {
		GeneratorKind::Impure => {
			generate_impure(config, display_name, secret, default_generator, owners).await
		}
	}
}
async fn generate_shared(
	config: &Config,
	display_name: &str,
	secret: Field,
	expected_owners: Vec<String>,
) -> Result<FleetSharedSecret> {
	// let owners: Vec<String> = nix_go_json!(secret.expectedOwners);
	Ok(FleetSharedSecret {
		secret: generate(config, display_name, secret, &expected_owners).await?,
		owners: expected_owners,
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

				let recipients = config.recipients(machines.clone()).await?;

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
				let secret = config.shared_secret(&name)?;
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

				let config_field = &config.config_unchecked_field;
				let field = nix_go!(config_field.sharedSecrets[{ name }]);

				let updated = update_owner_set(
					&name,
					config,
					secret,
					field,
					&target_machines,
					&prefer_identities,
				)
				.await?;
				config.replace_shared(name, updated);
			}
			Secret::Regenerate { prefer_identities } => {
				info!("checking for secrets to regenerate");
				{
					let _span = info_span!("shared").entered();
					let expected_shared_set = config
						.list_configured_shared()
						.await?
						.into_iter()
						.collect::<HashSet<_>>();
					let shared_set = config.list_shared().into_iter().collect::<HashSet<_>>();
					for missing in expected_shared_set.difference(&shared_set) {
						let config_field = &config.config_unchecked_field;
						let secret = nix_go!(config_field.sharedSecrets[{ missing }]);
						let expected_owners: Option<Vec<String>> =
							nix_go_json!(secret.expectedOwners);
						let Some(expected_owners) = expected_owners else {
							// TODO: Might still need to regenerate
							continue;
						};
						info!("generating secret: {missing}");
						let shared = generate_shared(config, missing, secret, expected_owners)
							.in_current_span()
							.await?;
						config.replace_shared(missing.to_string(), shared)
					}
				}
				for host in config.list_hosts().await? {
					let _span = info_span!("host", host = host.name).entered();
					let expected_set = host
						.list_configured_secrets()
						.in_current_span()
						.await?
						.into_iter()
						.collect::<HashSet<_>>();
					let stored_set = config
						.list_secrets(&host.name)
						.into_iter()
						.collect::<HashSet<_>>();
					for missing in expected_set.difference(&stored_set) {
						info!("generating secret: {missing}");
						let secret = host.secret_field(missing).in_current_span().await?;
						let generated =
							match generate(config, missing, secret, &[host.name.clone()])
								.in_current_span()
								.await
							{
								Ok(v) => v,
								Err(e) => {
									error!("{e}");
									continue;
								}
							};
						config.insert_secret(&host.name, missing.to_string(), generated)
					}
				}
				let mut to_remove = Vec::new();
				for name in &config.list_shared() {
					info!("updating secret: {name}");
					let data = config.shared_secret(name)?;
					let config_field = &config.config_unchecked_field;
					let expected_owners: Vec<String> =
						nix_go_json!(config_field.sharedSecrets[{ name }].expectedOwners);
					if expected_owners.is_empty() {
						warn!("secret was removed from fleet config: {name}, removing from data");
						to_remove.push(name.to_string());
						continue;
					}

					let secret = nix_go!(config_field.sharedSecrets[{ name }]);
					config.replace_shared(
						name.to_owned(),
						update_owner_set(
							&name,
							config,
							data,
							secret,
							&expected_owners,
							&prefer_identities,
						)
						.await?,
					);
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
