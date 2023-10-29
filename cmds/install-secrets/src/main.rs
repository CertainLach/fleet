use age::{ssh::Identity as SshIdentity, ssh::Recipient as SshRecipient, Decryptor};
use age::{Encryptor, Identity, Recipient};
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use log::{error, info, warn};
use nix::sys::stat::Mode;
use nix::unistd::{User, Group, chown};
use serde::{Deserialize, Deserializer};
use std::fmt::{self, Display};
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write};
use std::iter;
use std::os::unix::prelude::PermissionsExt;
use std::path::Path;
use std::str::{from_utf8, FromStr};
use std::{collections::HashMap, path::PathBuf};

#[derive(Clone, Debug)]
struct SecretWrapper(Vec<u8>);
impl Display for SecretWrapper {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let encoded = z85::encode(&self.0);
		write!(f, "{encoded}")
	}
}
impl FromStr for SecretWrapper {
	type Err = z85::DecodeError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		z85::decode(s).map(Self)
	}
}
impl<'de> Deserialize<'de> for SecretWrapper {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let v = String::deserialize(deserializer)?;
		let de = z85::decode(v).map_err(|err| serde::de::Error::custom(err.to_string()))?;
		Ok(Self(de))
	}
}

#[derive(Parser)]
#[clap(author)]
enum Opts {
	/// Install secrets from json specification
	Install { data: PathBuf },
	/// Reencrypt secret using host key, outputting in z85 encoded string
	Reencrypt {
		#[clap(long)]
		secret: SecretWrapper,
		#[clap(long)]
		targets: Vec<String>,
	},
	/// Decrypt secret using host key, outputting in z85 encoded string
	Decrypt {
		#[clap(long)]
		secret: SecretWrapper,
		/// Shoult decoded output be printed as plaintext, instead of z85?
		#[clap(long)]
		plaintext: bool,
	},
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataItem {
	group: String,
	mode: String,
	owner: String,

	secret: Option<SecretWrapper>,
	public: Option<String>,

	public_path: PathBuf,
	stable_public_path: PathBuf,

	secret_path: PathBuf,
	stable_secret_path: PathBuf,
}

type Data = HashMap<String, DataItem>;

fn decrypt(input: &SecretWrapper, identity: &dyn Identity) -> Result<Vec<u8>> {
	let mut input = Cursor::new(&input.0);
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
	Ok(decrypted)
}
fn encrypt(input: &[u8], targets: Vec<String>) -> Result<SecretWrapper> {
	let recipients = targets
		.into_iter()
		.map(|t| {
			SshRecipient::from_str(&t).map_err(|e| anyhow!("failed to parse recipient: {e:?}"))
		})
		.collect::<Result<Vec<SshRecipient>>>()?;
	let recipients = recipients
		.into_iter()
		.map(|v| Box::new(v) as Box<dyn Recipient + Send>)
		.collect::<Vec<_>>();
	let mut encrypted = vec![];
	let mut encryptor = Encryptor::with_recipients(recipients)
		.expect("recipients provided")
		.wrap_output(&mut encrypted)
		.expect("constructor should not fail");
	io::copy(&mut Cursor::new(input), &mut encryptor).expect("copy should not fail");
	encryptor.finish().context("failed to finish encryption")?;
	Ok(SecretWrapper(encrypted))
}

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
	let decrypted = decrypt(secret, identity)?;
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

fn host_identity() -> anyhow::Result<SshIdentity> {
	let identity = SshIdentity::from_buffer(
		&mut Cursor::new(
			fs::read("/etc/ssh/ssh_host_ed25519_key").context("failed to read host private key")?,
		),
		None,
	)
	.context("failed to parse identity")?;
	Ok(identity)
}

fn install(data: &Path) -> anyhow::Result<()> {
	let data = fs::read(data).context("failed to read secrets data")?;
	let data_str = from_utf8(&data).context("failed to read data to string")?;
	let data: Data = serde_json::from_str(data_str).context("failed to parse data")?;

	if !fs::metadata("/run/secrets")
		.map(|m| m.is_dir())
		.unwrap_or(false)
	{
		fs::create_dir("/run/secrets").context("failed to create secrets directory")?;
	}

	let identity = host_identity()?;

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

fn main() -> anyhow::Result<()> {
	env_logger::Builder::new()
		.filter_level(log::LevelFilter::Info)
		.init();

	let opts = Opts::parse();

	match opts {
		Opts::Install { data } => install(&data),
		Opts::Reencrypt { secret, targets } => {
			let identity = host_identity()?;
			let decrypted = decrypt(&secret, &identity).context("during decryption")?;
			let encrypted = encrypt(&decrypted, targets).context("during re-encryption")?;

			println!("{encrypted}");
			Ok(())
		}
		Opts::Decrypt { secret, plaintext } => {
			let identity = host_identity()?;
			let decrypted = decrypt(&secret, &identity).context("during decryption")?;

			if plaintext {
				let s = String::from_utf8(decrypted).context("output is not utf8")?;
				print!("{s}");
			} else {
				println!("{}", SecretWrapper(decrypted));
			}
			Ok(())
		}
	}
}
