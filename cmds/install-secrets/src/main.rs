use age::Decryptor;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use log::{error, warn};
use nix::fcntl::{renameat2, RenameFlags};
use nix::sys::stat::Mode;
use nix::unistd::{chown, Group, User};
use serde::{Deserialize, Deserializer};
use std::fs::{self, DirBuilder};
use std::io::{self, Cursor, Read};
use std::iter;
use std::os::unix::prelude::PermissionsExt;
use std::str::from_utf8;
use std::{
	collections::HashMap,
	os::unix::fs::DirBuilderExt,
	path::{Path, PathBuf},
};

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
	public: String,

	secret_hash: String,
	public_path: String,
}

fn from_z85<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
where
	D: Deserializer<'de>,
{
	use serde::de::Error;
	if let Some(v) = <Option<String>>::deserialize(deserializer)? {
		Ok(Some(
			z85::decode(&v).map_err(|err| Error::custom(err.to_string()))?,
		))
	} else {
		Ok(None)
	}
}

type Data = HashMap<String, DataItem>;

fn init_secret(
	identity: &age::ssh::Identity,
	dir: &Path,
	name: &str,
	value: DataItem,
) -> Result<()> {
	if value.secret.is_none() {
		return Ok(());
	}
	let secret = value.secret.as_ref().unwrap();

	let mut path = dir.to_path_buf();
	path.push(name);
	if path.strip_prefix(&dir).is_err() {
		bail!("found escaping name");
	}

	let secret_dir = path
		.parent()
		.expect("path is in tempdir, so it should have parent");

	if secret_dir != dir {
		DirBuilder::new()
			.recursive(true)
			// o: xrw
			// g: xr
			// a: xr
			.mode(0o755)
			.create(
				path.parent()
					.expect("path is in tempdir, so it should have parent"),
			)
			.context("failed to create secret directory")?;
	}

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
	let mut tempfile =
		tempfile::NamedTempFile::new_in(secret_dir).context("failed to create tempfile")?;
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

	io::copy(&mut Cursor::new(decrypted), &mut tempfile)
		.context("failed to write decrypted file")?;

	// Make file owned by specified user and group, then change mode
	chown(tempfile.path(), Some(user.uid), Some(group.gid))
		.context("failed to apply user/group")?;
	fs::set_permissions(tempfile.path(), fs::Permissions::from_mode(mode.bits())).unwrap();
	tempfile.persist(path).context("failed to persist")?;

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

	let tempdir = tempfile::tempdir_in("/run/").context("failed to create secrets tempdir")?;

	let identity = age::ssh::Identity::from_buffer(
		&mut Cursor::new(
			fs::read("/etc/ssh/ssh_host_ed25519_key").context("failed to read host private key")?,
		),
		None,
	)
	.context("failed to parse identity")?;

	let mut failed = false;
	for (name, value) in data {
		if let Err(e) = init_secret(&identity, tempdir.path(), &name, value) {
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

	if fs::metadata("/run/secrets")
		.map(|m| m.is_dir())
		.unwrap_or(false)
	{
		// Already linked
		renameat2(
			None,
			tempdir.path(),
			None,
			"/run/secrets",
			RenameFlags::RENAME_EXCHANGE,
		)
		.context("failed to exchange secret directories")?;
		if tempdir.close().is_err() {
			warn!("failed to unlink old secrets");
		}
	} else {
		// Link now
		let persisted = tempdir.into_path();
		fs::rename(&persisted, "/run/secrets").context("failed to link secret directory")?;
	}
	Ok(())
}
