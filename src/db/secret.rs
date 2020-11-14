use crate::nix::{NixBuild, NixEval, SECRETS_ATTRIBUTE};
use anyhow::{bail, Result};
use log::info;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
	collections::{BTreeMap, BTreeSet, HashMap},
	time::Instant,
	time::SystemTime,
};
use time::{Duration, PrimitiveDateTime};

use super::{db::DbData, keys::KeyDb};

#[derive(Serialize, Deserialize, Debug)]
pub struct SecretListData {
	pub owners: BTreeSet<String>,
	#[serde(rename = "expireIn")]
	renew_in: Option<u64>,
}
pub fn list_secrets() -> Result<HashMap<String, SecretListData>> {
	NixEval::new(format!("{}", SECRETS_ATTRIBUTE))
		.apply(
			r#"
				s: (builtins.mapAttrs (n: {owners, expireIn, ...}: {
					inherit owners expireIn;
				}) s)
			"#
			.into(),
		)
		.run_json()
}

struct ReadableDate(PrimitiveDateTime);
impl Serialize for ReadableDate {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		serializer.serialize_str(&self.0.to_string())
	}
}
impl<'de> Deserialize<'de> for ReadableDate {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		Ok(Self(
			PrimitiveDateTime::parse(String::deserialize(deserializer)?, "%F %T").unwrap(),
		))
	}
}
impl From<PrimitiveDateTime> for ReadableDate {
	fn from(d: PrimitiveDateTime) -> Self {
		Self(d)
	}
}
impl From<ReadableDate> for PrimitiveDateTime {
	fn from(d: ReadableDate) -> Self {
		d.0
	}
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SecretData {
	created_at: ReadableDate,
	renew_at: Option<ReadableDate>,
	owners: BTreeSet<String>,

	public_data: BTreeMap<String, String>,
	private_files: BTreeMap<String, String>,
}
impl SecretData {
	fn should_renew(&self) -> bool {
		if let Some(renew_at) = &self.renew_at {
			let now: PrimitiveDateTime = SystemTime::now().into();
			renew_at.0 <= now
		} else {
			false
		}
	}
	fn is_valid(&self, data: &SecretListData) -> bool {
		self.owners == data.owners
	}
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NixDataValue {
	data: BTreeMap<String, String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NixData {
	secrets: BTreeMap<String, NixDataValue>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct SecretDb {
	secrets: BTreeMap<String, SecretData>,
}
impl DbData for SecretDb {
	const DB_NAME: &'static str = "secrets";
}

impl SecretDb {
	// Secrets are generated on machine running fleet command
	pub fn generate_secret(
		&mut self,
		keys: &KeyDb,
		secret: &str,
		data: &SecretListData,
	) -> Result<()> {
		let mut rage_keys = String::new();
		for (i, owner) in data.owners.iter().enumerate() {
			if i != 0 {
				rage_keys.push(' ');
			}
			rage_keys.push_str("--recipient \"");
			rage_keys.push_str(&keys.get_host_key(&owner)?);
			rage_keys.push('"')
		}
		let created_at: PrimitiveDateTime = SystemTime::now().into();
		let renew_at = data
			.renew_in
			.map(|hours| created_at + Duration::hours(hours as i64));
		let built = NixBuild::new(format!("{}.{}.generator", SECRETS_ATTRIBUTE, secret))
			.env("RAGE_KEYS".into(), rage_keys)
			.env("IMPURITY_SOURCE".into(), format!("{:?}", Instant::now()))
			.run()?;
		let path = built.path().to_owned();
		let mut secret_data = SecretData {
			created_at: created_at.into(),
			renew_at: renew_at.map(|v| v.into()),
			owners: data.owners.clone(),
			public_data: BTreeMap::new(),
			private_files: BTreeMap::new(),
		};
		for file in std::fs::read_dir(path)? {
			let entry = file?;
			if !entry.file_type()?.is_file() {
				bail!("Secret generator should produce files, not directories");
			}
			let name = entry.file_name();
			let name = name
				.to_str()
				.ok_or(anyhow::anyhow!("file name should be utf-8"))?;
			let value = String::from_utf8(std::fs::read(entry.path())?)?;
			if let Some(name) = name.strip_prefix("pub_") {
				secret_data.public_data.insert(name.into(), value);
			} else {
				secret_data.private_files.insert(name.into(), value);
			}
		}
		self.secrets.insert(secret.into(), secret_data);
		Ok(())
	}
	pub fn need_to_generate(&self, secret: &str, data: &SecretListData) -> Result<bool> {
		let secret = self.secrets.get(secret);
		if secret.is_none() {
			return Ok(true);
		}
		let secret = secret.unwrap();

		if secret.should_renew() {
			return Ok(true);
		}

		if !secret.is_valid(&data) {
			return Ok(true);
		}

		Ok(false)
	}
	pub fn ensure_generated(
		&mut self,
		keys: &KeyDb,
		secret: &str,
		data: &SecretListData,
	) -> Result<()> {
		if self.need_to_generate(secret, data)? {
			info!("Generating secret {}", secret);
			self.generate_secret(keys, secret, data)?;
		}

		Ok(())
	}
	pub fn generate_nix_data(&self) -> Result<String> {
		let mut out = BTreeMap::new();
		for (host, secrets) in &self.secrets {
			out.insert(
				host.to_owned(),
				NixDataValue {
					data: secrets
						.public_data
						.clone()
						.iter()
						.map(|(k, v)| (k.to_owned(), v.trim().to_owned()))
						.collect(),
				},
			);
		}
		Ok(serde_json::to_string(&out)?)
	}

	pub fn has_secret(&self, secret: &str) -> bool {
		self.secrets.contains_key(secret)
	}

	pub fn remove_secret(&mut self, secret: &str) {
		self.secrets.remove(secret);
	}
}
