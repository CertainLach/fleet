use std::{
	collections::{BTreeMap, BTreeSet, HashSet},
	ffi::OsString,
	io::{self, stdin, stdout, Read, Write},
	path::PathBuf,
};

use anyhow::{anyhow, bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use crossterm::{terminal, tty::IsTty};
use fleet_shared::SecretData;
use itertools::Itertools;
use owo_colors::OwoColorize;
use serde::Deserialize;
use tabled::{Table, Tabled};
use tokio::{fs::read, process::Command};
use tracing::{error, info, info_span, warn, Instrument};

use crate::{
	better_nix_eval::Field,
	fleetdata::{encrypt_secret_data, FleetSecret, FleetSecretPart, FleetSharedSecret},
	host::Config,
	nix_go, nix_go_json,
};

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
		/// How to name public secret part
		#[clap(long, default_value = "public")]
		public_name: String,
		/// Load public part from specified file
		#[clap(long)]
		public_file: Option<PathBuf>,

		/// Create a notification on secret expiration
		#[clap(long)]
		expires_at: Option<DateTime<Utc>>,

		/// Secret with this name already exists, override its value while keeping the same owners.
		#[clap(long)]
		re_add: bool,

		#[clap(default_value = "secret")]
		part_name: String,
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
		/// Secret public part
		#[clap(long)]
		public: Option<String>,
		/// How to name public secret part
		#[clap(long, default_value = "public")]
		public_name: String,
		/// Load public part from specified file
		#[clap(long)]
		public_file: Option<PathBuf>,

		#[clap(default_value = "secret")]
		part_name: String,
	},
	/// Read secret from remote host, requires sudo on said host
	Read {
		name: String,
		machine: String,

		#[clap(default_value = "secret")]
		part_name: String,
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

		#[clap(default_value = "secret")]
		part_name: String,
	},
	Regenerate {
		/// Which host should we use to decrypt, in case if reencryption is required, without
		/// regeneration
		#[clap(long)]
		prefer_identities: Vec<String>,
	},
	List {},
	Edit {
		name: String,
		machine: String,

		#[clap(default_value = "secret")]
		part: String,

		#[clap(long)]
		add: bool,
	},
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

		for (part_name, part) in secret.secret.parts.iter_mut() {
			let _span = info_span!("part reencryption", part_name);
			if !part.raw.encrypted {
				continue;
			}
			let host = config.host(identity_holder).await?;
			let encrypted = host
				.reencrypt(part.raw.clone(), updated_set.to_vec())
				.await?;
			part.raw = encrypted;
		}

		secret.owners = updated_set.to_vec();
		Ok(secret)
	}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum GeneratorKind {
	Impure,
	Pure,
}

async fn generate_pure(
	_config: &Config,
	_display_name: &str,
	_secret: Field,
	_default_generator: Field,
	_owners: &[String],
) -> Result<FleetSecret> {
	bail!("pure generators are broken for now")
}
async fn generate_impure(
	config: &Config,
	_display_name: &str,
	secret: Field,
	default_generator: Field,
	owners: &[String],
) -> Result<FleetSecret> {
	let generator = nix_go!(secret.generator);
	let on: Option<String> = nix_go_json!(default_generator.impureOn);

	let host = if let Some(on) = &on {
		config.host(on).await?
	} else {
		config.local_host()
	};
	let on_pkgs = host.pkgs().await?;
	let call_package = nix_go!(on_pkgs.callPackage);
	let mk_encrypt_secret = nix_go!(on_pkgs.mkEncryptSecret);

	let mut recipients = Vec::new();
	for owner in owners {
		let key = config.key(owner).await?;
		recipients.push(key);
	}
	let encrypt = nix_go!(mk_encrypt_secret(Obj {
		recipients: { recipients },
	}));

	let generator = nix_go!(call_package(generator)(Obj {
		encrypt,
		// rustfmt_please_newline
	}));

	let generator = generator.build().await?;
	let generator = generator
		.get("out")
		.ok_or_else(|| anyhow!("missing generateImpure out"))?;
	let generator = host.remote_derivation(generator).await?;

	let out_parent = host.mktemp_dir().await?;
	let out = format!("{out_parent}/out");

	let mut gen = host.cmd(generator).await?;
	gen.env("out", &out);
	if on.is_none() {
		// This path is local, thus we can feed `OsString` directly to env var... But I don't think that's necessary to handle.
		let project_path: String = config
			.directory
			.clone()
			.into_os_string()
			.into_string()
			.map_err(|s| anyhow!("fleet project path is not utf-8: {s:?}"))?;
		gen.env("FLEET_PROJECT", project_path);
	}
	gen.run().await.context("impure generator")?;

	{
		let marker = host.read_file_text(format!("{out}/marker")).await?;
		ensure!(marker == "SUCCESS", "generation not succeeded");
	}

	let mut parts = BTreeMap::new();
	for part in host.read_dir(&out).await? {
		if part == "created_at" || part == "expired_at" || part == "marker" {
			continue;
		}
		let contents: SecretData = host
			.read_file_text(format!("{out}/{part}"))
			.await?
			.parse()
			.map_err(|e| anyhow!("failed to decode secret {out:?} part {part:?}: {e}"))?;
		parts.insert(part.to_owned(), FleetSecretPart { raw: contents });
	}

	let created_at = host.read_file_value(format!("{out}/created_at")).await?;
	let expires_at = host.read_file_value(format!("{out}/expires_at")).await.ok();

	Ok(FleetSecret {
		created_at,
		expires_at,
		parts,
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
	let default_generator = nix_go!(default_call_package(generator)(Obj {
		encrypt: { "exit 1" },
		// rustfmt_please_newline
	}));

	let kind: GeneratorKind = nix_go_json!(default_generator.generatorKind);

	match kind {
		GeneratorKind::Impure => {
			generate_impure(config, display_name, secret, default_generator, owners).await
		}
		GeneratorKind::Pure => {
			generate_pure(config, display_name, secret, default_generator, owners).await
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
) -> Result<Option<SecretData>> {
	Ok(match (public, public_file) {
		(Some(v), None) => Some(SecretData {
			data: v.into(),
			encrypted: false,
		}),
		(None, Some(v)) => Some(SecretData {
			data: read(v).await?,
			encrypted: false,
		}),
		(Some(_), Some(_)) => {
			bail!("only public or public_file should be set")
		}
		(None, None) => None,
	})
}

async fn parse_secret() -> Result<Option<Vec<u8>>> {
	let mut input = vec![];
	io::stdin().read_to_end(&mut input)?;
	if input.is_empty() {
		Ok(None)
	} else {
		Ok(Some(input))
	}
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
				public_name,
				public_file,
				expires_at,
				re_add,
				part_name,
			} => {
				// TODO: Forbid updating secrets with set expectedOwners (= not user-managed).

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

				let mut parts = BTreeMap::new();

				let mut input = vec![];
				io::stdin().read_to_end(&mut input)?;

				if !input.is_empty() {
					let encrypted = encrypt_secret_data(recipients, input)
						.ok_or_else(|| anyhow!("no recipients provided"))?;
					parts.insert(part_name, FleetSecretPart { raw: encrypted });
				}

				if let Some(public) = parse_public(public, public_file).await? {
					parts.insert(public_name, FleetSecretPart { raw: public });
				}

				config.replace_shared(
					name,
					FleetSharedSecret {
						owners: machines,
						secret: FleetSecret {
							created_at: Utc::now(),
							expires_at,
							parts,
						},
					},
				);
			}
			Secret::Add {
				machine,
				name,
				force,
				public,
				public_name,
				public_file,
				part_name,
			} => {
				if config.has_secret(&machine, &name) && !force {
					bail!("secret already defined");
				}

				let mut parts = BTreeMap::new();

				if let Some(secret) = parse_secret().await? {
					let recipient = config.recipient(&machine).await?;
					let encrypted =
						encrypt_secret_data(vec![recipient], secret).expect("recipient provided");
					parts.insert(part_name, FleetSecretPart { raw: encrypted });
				}

				if let Some(public) = parse_public(public, public_file).await? {
					parts.insert(public_name, FleetSecretPart { raw: public });
				};

				config.insert_secret(
					&machine,
					name,
					FleetSecret {
						created_at: Utc::now(),
						expires_at: None,
						parts,
					},
				);
			}
			#[allow(clippy::await_holding_refcell_ref)]
			Secret::Read {
				name,
				machine,
				part_name,
			} => {
				let secret = config.host_secret(&machine, &name)?;
				let Some(secret) = secret.parts.get(&part_name) else {
					bail!("no part {part_name} in secret {name}");
				};
				let data = if secret.raw.encrypted {
					let host = config.host(&machine).await?;
					host.decrypt(secret.raw.clone()).await?
				} else {
					secret.raw.data.clone()
				};

				stdout().write_all(&data)?;
			}
			Secret::UpdateShared {
				name,
				machines,
				add_machines,
				remove_machines,
				prefer_identities,
				part_name,
			} => {
				// TODO: Forbid updating secrets with set expectedOwners (= not user-managed).

				let secret = config.shared_secret(&name)?;
				if secret.secret.parts.get(&part_name).is_none() {
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
					if config.should_skip(&host.name) {
						continue;
					}

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
									error!("{e:?}");
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
							name,
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
			Secret::Edit {
				name,
				machine,
				part,
				add,
			} => {
				let secret = config.host_secret(&machine, &name)?;
				if let Some(data) = secret.parts.get(&part) {
					let host = config.host(&machine).await?;
					let secret = host.decrypt(data.raw.clone()).await?;
					String::from_utf8(secret).context("secret is not utf8")?
				} else if add {
					String::new()
				} else {
					bail!("part {part} not found in secret {name}. Did you mean to `--add` it?");
				};
			}
		}
		Ok(())
	}
}

async fn edit_temp_file(
	builder: tempfile::Builder<'_, '_>,
	r: Vec<u8>,
	header: &str,
	comment: &str,
) -> Result<(Vec<u8>, Option<String>), anyhow::Error> {
	if !stdin().is_tty() {
		// TODO: Also try to open /dev/tty directly?
		bail!("stdin is not tty, can't open editor");
	}

	use std::fmt::Write;
	let mut file = builder.tempfile()?;

	let mut full_header = String::new();
	let mut had = false;
	for line in header.trim_end().lines() {
		had = true;
		writeln!(&mut full_header, "{comment}{line}")?;
	}
	if had {
		writeln!(&mut full_header, "{}", comment.trim_end())?;
	}
	writeln!(
		&mut full_header,
		"{comment}Do not touch this header! It will be removed automatically"
	)?;

	file.write_all(full_header.as_bytes())?;
	file.write_all(&r)?;

	let abs_path = file.into_temp_path();
	let editor = std::env::var_os("VISUAL")
		.or_else(|| std::env::var_os("EDITOR"))
		.unwrap_or_else(|| "vi".into());
	let editor_args = shlex::bytes::split(editor.as_encoded_bytes())
		.ok_or_else(|| anyhow!("EDITOR env var has wrong syntax"))?;
	let editor_args = editor_args
		.into_iter()
		.map(|v| {
			// Only ASCII subsequences are replaced
			unsafe { OsString::from_encoded_bytes_unchecked(v) }
		})
		.collect_vec();
	let Some((editor, args)) = editor_args.split_first() else {
		bail!("EDITOR env var has no command");
	};
	let mut command = Command::new(editor);
	command.args(args);

	let path_arg = abs_path.canonicalize()?;

	// TODO: Save full state, using tcget/_getmode/_setmode
	let was_raw = terminal::is_raw_mode_enabled()?;
	terminal::enable_raw_mode()?;

	let status = command.arg(path_arg).status().await;

	if !was_raw {
		terminal::disable_raw_mode()?;
	}

	let success = match status {
		Ok(s) => s.success(),
		Err(e) if e.kind() == io::ErrorKind::NotFound => {
			bail!("editor not found")
		}
		Err(e) => bail!("editor spawn error: {e}"),
	};

	let mut file = std::fs::read(&abs_path).context("read editor output")?;
	let Some(v) = file.strip_prefix(full_header.as_bytes()) else {
		todo!();
	};
	todo!();

	// Ok((success, abs_path))
}
