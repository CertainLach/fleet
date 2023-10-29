use anyhow::Result;
use chrono::{DateTime, Utc};
use nixlike::format_nix;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use tempfile::TempDir;
use tokio::{
	fs::{self, File},
	io::AsyncWriteExt,
	process::Command,
};

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HostData {
	#[serde(default)]
	#[serde(skip_serializing_if = "String::is_empty")]
	pub encryption_key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetData {
	#[serde(default)]
	pub hosts: BTreeMap<String, HostData>,
	#[serde(default)]
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	pub shared_secrets: BTreeMap<String, FleetSharedSecret>,
	#[serde(default)]
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	pub host_secrets: BTreeMap<String, BTreeMap<String, FleetSecret>>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
#[must_use]
pub struct FleetSharedSecret {
	pub owners: Vec<String>,
	#[serde(flatten)]
	pub secret: FleetSecret,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
#[must_use]
pub struct FleetSecret {
	#[serde(default = "Utc::now")]
	pub created_at: DateTime<Utc>,
	#[serde(default)]
	#[serde(skip_serializing_if = "Option::is_none", alias = "expire_at")]
	pub expires_at: Option<DateTime<Utc>>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub public: Option<String>,
	#[serde(
		default,
		skip_serializing_if = "Vec::is_empty",
		serialize_with = "as_z85",
		deserialize_with = "from_z85"
	)]
	pub secret: Vec<u8>,
}

fn as_z85<S>(key: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
	S: Serializer,
{
	serializer.serialize_str(&z85::encode(key))
}

fn from_z85<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
	D: Deserializer<'de>,
{
	use serde::de::Error;
	String::deserialize(deserializer)
		.and_then(|string| z85::decode(string).map_err(|err| Error::custom(err.to_string())))
}

/// Isn't used yet
#[allow(dead_code)]
pub async fn dummy_flake() -> Result<TempDir> {
	let data_str = fs::read_to_string("fleet.nix").await?;

	let mut cmd = Command::new("nix");
	cmd.arg("flake").arg("metadata").arg("--json");

	let flake_dir = tempfile::tempdir()?;
	let mut flake_nix = flake_dir.path().to_path_buf();
	flake_nix.push("flake.nix");
	// flake_dir

	File::create(&flake_nix)
		.await?
		.write_all(
			format_nix(&format!(
				"
						{{
							outputs = {{self, ...}}: {{
								data = {data_str};
							}};
						}}
					"
			))
			.as_bytes(),
		)
		.await?;

	// std::thread::sleep(Duration::MAX);
	// flake_dir.close()
	// FIXME
	dbg!(&flake_nix);
	Ok(flake_dir)
}
