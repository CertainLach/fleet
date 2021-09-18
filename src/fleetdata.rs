use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Default)]
pub struct HostData {
	#[serde(default)]
	pub encryption_key: String,
	#[serde(default)]
	pub encrypted_secrets: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub struct FleetData {
	#[serde(default)]
	pub hosts: BTreeMap<String, HostData>,
}
