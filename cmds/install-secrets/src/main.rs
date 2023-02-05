use age::Decryptor;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use log::{error, info, warn};
use nix::sys::stat::Mode;
use nix::unistd::{chown, Group, User};
use serde::{Deserialize, Deserializer};
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write};
use std::iter;
use std::os::unix::prelude::PermissionsExt;
use std::str::from_utf8;
use std::{collections::HashMap, path::PathBuf};

#[derive(Parser)]
#[clap(author)]
struct Opts {
	data: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataItem {
	group: String,
	mode: String,
	owner: String,

	#[serde(deserialize_with = "from_z85")]
	secret: Option<Vec<u8>>,
	public: Option<String>,

	public_path: PathBuf,
	stable_public_path: PathBuf,

	secret_path: PathBuf,
	stable_secret_path: PathBuf,
}

fn from_z85<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
where
	D: Deserializer<'de>,
{
	use serde::de::Error;
	if let Some(v) = <Option<String>>::deserialize(deserializer)? {
		Ok(Some(
			z85::decode(v).map_err(|err| Error::custom(err.to_string()))?,
		))
	} else {
		Ok(None)
	}
}

type Data = HashMap<String, DataItem>;

fn init_secret(identity: &age::ssh::Identity, value: DataItem) -> Result<()> {
	if let Some(public) = &value.public {
		let mut hashed = File::create(&value.public_path)?;
		let stable_dir = value.stable_public_path.parent().expect("not root");
		let mut stable_temp =
			tempfile::NamedTempFile::new_in(stable_dir).context("failed to create tempfile")?;
		hashed.write_all(public.as_bytes())?;
		stable_temp.write_all(public.as_bytes())?;
		stable_temp.flush()?;
		fs::set_permissions(stable_temp.path(), fs::Permissions::from_mode(0o444))
			.context("perm")?;
		fs::set_permissions(&value.public_path, fs::Permissions::from_mode(0o444))
			.context("perm")?;

		stable_temp
			.persist(value.stable_public_path)
			.context("failed to persist")?;
	}
	if value.secret.is_none() {
		info!("no secret data found");
		return Ok(());
	}
	let secret = value.secret.as_ref().unwrap();

	let mode = Mode::from_bits(
		u32::from_str_radix(&value.mode, 8).context("failed to parse mode as octal")?,
	)
	.context("failed to parse mode")?;
	let user = User::from_name(&value.owner)
		.context("failed to get user")?
		.ok_or_else(|| anyhow!("user not found"))?;
	let group = Group::from_name(&value.group)
		.context("failed to get group")?
		.ok_or_else(|| anyhow!("group not found"))?;

	let stable_dir = value.stable_secret_path.parent().expect("not root");
	let mut stable_temp =
		tempfile::NamedTempFile::new_in(stable_dir).context("failed to create tempfile")?;
	let mut hashed = File::create(&value.secret_path)?;

	// File is owned by root, and only root can modify it
	let decrypted = {
		let mut input = Cursor::new(&secret);
		let decryptor = Decryptor::new(&mut input).context("failed to init decryptor")?;
		let decryptor = match decryptor {
			Decryptor::Recipients(r) => r,
			Decryptor::Passphrase(_) => bail!("should be recipients"),
		};
		let mut decryptor = decryptor
			.decrypt(iter::once(identity as &dyn age::Identity))
			.context("failed to decrypt, wrong key?")?;

		let mut decrypted = Vec::new();
		decryptor
			.read_to_end(&mut decrypted)
			.context("failed to decrypt")?;
		decrypted
	};
	if decrypted.is_empty() {
		warn!("secret is decoded as empty, something is broken?");
	}

	io::copy(&mut Cursor::new(&decrypted), &mut stable_temp)
		.context("failed to write decrypted file")?;
	io::copy(&mut Cursor::new(decrypted), &mut hashed).context("failed to write decrypted file")?;

	// Make file owned by specified user and group, then change mode
	chown(stable_temp.path(), Some(user.uid), Some(group.gid))
		.context("failed to apply user/group")?;
	chown(&value.secret_path, Some(user.uid), Some(group.gid))
		.context("failed to apply user/group")?;
	fs::set_permissions(stable_temp.path(), fs::Permissions::from_mode(mode.bits())).unwrap();
	fs::set_permissions(&value.secret_path, fs::Permissions::from_mode(mode.bits())).unwrap();
	stable_temp
		.persist(value.stable_secret_path)
		.context("failed to persist")?;

	Ok(())
}

fn main() -> anyhow::Result<()> {
	env_logger::Builder::new()
		.filter_level(log::LevelFilter::Info)
		.init();

	let opts = Opts::parse();
	let data = fs::read(&opts.data).context("failed to read secrets data")?;
	let data_str = from_utf8(&data).context("failed to read data to string")?;
	let data: Data = serde_json::from_str(data_str).context("failed to parse data")?;

	if !fs::metadata("/run/secrets")
		.map(|m| m.is_dir())
		.unwrap_or(false)
	{
		fs::create_dir("/run/secrets").context("failed to create secrets directory")?;
	}

	let identity = age::ssh::Identity::from_buffer(
		&mut Cursor::new(
			fs::read("/etc/ssh/ssh_host_ed25519_key").context("failed to read host private key")?,
		),
		None,
	)
	.context("failed to parse identity")?;

	let mut failed = false;
	for (name, value) in data {
		info!("initializing secret {name}");
		if let Err(e) = init_secret(&identity, value) {
			error!(
				"{:?}",
				e.context(format!("failed to initialize secret {}", name))
			);
			failed = true;
		}
	}
	if failed {
		bail!("one or more secrets failed");
	}

	Ok(())
}
