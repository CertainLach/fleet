use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HostData {
	#[serde(default)]
	#[serde(skip_serializing_if = "String::is_empty")]
	pub encryption_key: String,
}

#[derive(Serialize, Deserialize)]
pub struct FleetData {
	#[serde(default)]
	pub hosts: BTreeMap<String, HostData>,
	#[serde(default)]
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	pub secret: BTreeMap<String, FleetSecret>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetSecret {
	pub owners: Vec<String>,
	#[serde(default)]
	#[serde(skip_serializing_if = "Option::is_none")]
	pub expire_at: Option<DateTime<Utc>>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub public: Option<String>,
	pub secret: String,
}
