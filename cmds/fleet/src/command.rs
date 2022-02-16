use std::{ffi::OsStr, process::Stdio};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{
	de::{DeserializeOwned, Visitor},
	Deserialize,
};
use tokio::{process::Command, select};
use tokio_util::codec::{BytesCodec, FramedRead, LinesCodec};
use tracing::{info, warn};

#[async_trait]
pub trait CommandExt {
	async fn run_nix(&mut self) -> Result<()>;
	async fn run_nix_json<T: DeserializeOwned>(&mut self) -> Result<T>;
	async fn run_nix_string(&mut self) -> Result<String>;
	async fn run(&mut self) -> Result<()>;
	async fn run_json<T: DeserializeOwned>(&mut self) -> Result<T>;
	async fn run_string(&mut self) -> Result<String>;
	fn inherit_stdio(&mut self) -> &mut Self;
	fn ssh_on(host: impl AsRef<OsStr>, command: impl AsRef<OsStr>) -> Self;
}

#[derive(Debug)]
enum LogField {
	String(String),
	Num(u64),
}

impl<'de> Deserialize<'de> for LogField {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: serde::Deserializer<'de>,
	{
		struct StringOrNum;
		impl<'de> Visitor<'de> for StringOrNum {
			type Value = LogField;

			fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
				write!(f, "string or unsigned")
			}

			fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
			where
				E: serde::de::Error,
			{
				Ok(LogField::String(v.to_owned()))
			}

			fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
			where
				E: serde::de::Error,
			{
				Ok(LogField::Num(v))
			}
		}

		deserializer.deserialize_any(StringOrNum)
	}
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", tag = "action")]
#[allow(dead_code)]
enum NixLog {
	Msg {
		level: u32,
		msg: String,
		raw_msg: Option<String>,
	},
	Start {
		id: u64,
		level: u32,
		#[serde(default)]
		fields: Vec<LogField>,
		text: String,
		#[serde(rename = "type")]
		typ: u32,
	},
	Stop {
		id: u64,
	},
	Result {
		id: u64,
		#[serde(rename = "type")]
		typ: u32,
	},
}

#[async_trait]
impl CommandExt for Command {
	async fn run_nix(&mut self) -> Result<()> {
		self.run_nix_string().await.map(|_| ())
	}
	async fn run_nix_json<T: DeserializeOwned>(&mut self) -> Result<T> {
		let str = self.run_nix_string().await?;
		serde_json::from_str(&str).with_context(|| format!("{:?}", str))
	}

	async fn run_nix_string(&mut self) -> Result<String> {
		self.arg("--log-format").arg("internal-json");
		self.stderr(Stdio::piped());
		self.stdout(Stdio::piped());
		let mut child = self.spawn()?;
		let mut stderr = child.stderr.take().unwrap();
		let mut stdout = child.stdout.take().unwrap();
		let mut err = FramedRead::new(&mut stderr, LinesCodec::new());
		let mut out = FramedRead::new(&mut stdout, BytesCodec::new());

		// while let Some(line) = read.next().await? {}

		let mut out_buf = vec![];
		loop {
			select! {
				e = err.next() => {
					if let Some(e) = e {
						let e = e?;
						if let Some(e) = e.strip_prefix("@nix ") {

							let log: NixLog = match serde_json::from_str(e) {
								Ok(l) => l,
								Err(err) => {
									warn!("failed to parse nix log line {:?}: {}", e, err);
									continue;
								},
							};
							match log {
								NixLog::Msg { msg, raw_msg, .. } => {
									if !(msg.ends_with(" is dirty") && msg.contains("warning:") && msg.contains(" Git tree ")) {
										info!(target: "nix", "{}", raw_msg.unwrap_or(msg))
									}
								},
								NixLog::Start { ref fields, typ, .. } if typ == 105 && fields.len() >= 1 => {
									if let [LogField::String(drv), ..] = &fields[..] {
										info!(target: "nix","building {}", drv)
									} else {
										warn!("bad build log: {:?}", log)
									}
								},
								NixLog::Start { ref fields, typ, .. } if typ == 100 && fields.len() >= 3 => {
									if let [LogField::String(drv), LogField::String(from), LogField::String(to), ..] = &fields[..] {
										info!(target: "nix","copying {} {} -> {}", drv, from, to)
									} else {
										warn!("bad copy log: {:?}", log)
									}
								},
								NixLog::Start { text, typ, .. } if typ == 0 || typ == 102 || typ == 103 || typ == 104 => {
									if !text.is_empty() && text != "querying info about missing paths" && text != "copying 0 paths" {
										info!(target: "nix", "{}", text)
									}
								},
								NixLog::Start { text, level: 0, typ: 108, .. } if text == "" => {
									// Cache lookup? Coupled with copy log
								},
								NixLog::Start { text, level: 4, typ: 109, .. } if text.starts_with("querying info about ") => {
									// Cache lookup
								}
								NixLog::Start { text, level: 4, typ: 101, .. } if text.starts_with("downloading ") => {
									// NAR downloading, coupled with copy log
								}
								NixLog::Start { text, level: 1, typ: 111, .. } if text.starts_with("waiting for a machine to build ") => {
									// Useless repeating notification about build
								}
								NixLog::Stop { .. } => {},
								NixLog::Result { .. } => {},
								_ => warn!("unknown log: {:?}", log)
							};
						} else {
							warn!(target="nix","unknown: {}", e)
						}
					}
				},
				o = out.next() => {
					if let Some(o) = o {
						out_buf.extend_from_slice(&o?);
					}
				},
				code = child.wait() => {
					let code = code?;
					if !code.success() {
						anyhow::bail!("command ({:?}) failed with status {}", self, code);
					}
					break;
				}
			}
		}

		Ok(String::from_utf8(out_buf)?)
	}

	fn inherit_stdio(&mut self) -> &mut Self {
		self.stderr(Stdio::inherit());
		self
	}

	async fn run(&mut self) -> Result<()> {
		self.inherit_stdio();
		let out = self.output().await?;
		if !out.status.success() {
			anyhow::bail!("command ({:?}) failed with status {}", self, out.status);
		}
		Ok(())
	}

	async fn run_json<T: DeserializeOwned>(&mut self) -> Result<T> {
		let str = self.run_string().await?;
		serde_json::from_str(&str).with_context(|| format!("{:?}", str))
	}

	async fn run_string(&mut self) -> Result<String> {
		self.inherit_stdio();
		let out = self.output().await?;
		if !out.status.success() {
			anyhow::bail!("command ({:?}) failed with status {}", self, out.status);
		}
		Ok(String::from_utf8(out.stdout)?)
	}

	fn ssh_on(host: impl AsRef<OsStr>, command: impl AsRef<OsStr>) -> Self {
		let mut cmd = Command::new("ssh");
		cmd.arg(host).arg("--").arg(command);
		cmd
	}
}
