use std::{ffi::OsStr, process::Stdio, task::Poll};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{
	de::{DeserializeOwned, Visitor},
	Deserialize,
};
use tokio::{io::AsyncRead, process::Command, select};
use tokio_util::codec::{BytesCodec, FramedRead, LinesCodec};
use tracing::{info, warn};

fn escape_bash(input: &str, out: &mut String) {
	const TO_ESCAPE: &str = "$ !\"#&'()*,;<>?[\\]^`{|}";
	if input.chars().all(|c| !TO_ESCAPE.contains(c)) {
		out.push_str(input);
		return;
	}
	out.push('\'');
	for (i, v) in input.split('\'').enumerate() {
		if i != 0 {
			out.push_str("'\"'\"'");
		}
		out.push_str(v);
	}
	out.push('\'');
}
fn ostoutf8(os: impl AsRef<OsStr>) -> String {
	os.as_ref().to_str().expect("non-utf8 data").to_owned()
}
#[derive(Clone)]
pub struct MyCommand {
	command: String,
	args: Vec<String>,
	env: Vec<(String, String)>,
}
impl MyCommand {
	pub fn new(cmd: impl AsRef<OsStr>) -> Self {
		assert!(!cmd.as_ref().is_empty());
		Self {
			command: ostoutf8(cmd),
			args: vec![],
			env: vec![],
		}
	}
	fn into_args(self) -> Vec<String> {
		let mut out = Vec::new();
		if !self.env.is_empty() {
			out.push("env".to_owned());
			for (k, v) in self.env {
				assert!(!k.contains("="));
				out.push(format!("{k}={v}"));
			}
		}
		out.push(self.command);
		out.extend(self.args.into_iter());
		out
	}
	fn into_string(self) -> String {
		let mut out = String::new();
		if !self.env.is_empty() {
			out.push_str("env");
			for (k, v) in self.env {
				out.push(' ');
				assert!(!k.contains("="));
				escape_bash(&k, &mut out);
				out.push('=');
				escape_bash(&v, &mut out);
			}
		}
		if !out.is_empty() {
			out.push(' ');
		}
		escape_bash(&self.command, &mut out);
		for arg in self.args {
			out.push(' ');
			escape_bash(&arg, &mut out);
		}
		out
	}
	fn into_command(self) -> Command {
		let mut out = Command::new(self.command);
		out.args(self.args);
		for (k, v) in self.env {
			out.env(k, v);
		}
		out
	}
	pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
		let arg = arg.as_ref();
		self.args.push(ostoutf8(arg));
		self
	}
	pub fn eqarg(&mut self, arg: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
		let arg = arg.as_ref();
		let value = value.as_ref();
		let arg = ostoutf8(arg);
		let value = ostoutf8(value);
		self.arg(format!("{arg}={value}"));
		self
	}
	pub fn comparg(&mut self, arg: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
		self.arg(arg);
		self.arg(value);
		self
	}
	pub fn args<V: AsRef<OsStr>>(&mut self, args: impl IntoIterator<Item = V>) -> &mut Self {
		for arg in args.into_iter() {
			let arg = arg.as_ref();
			self.args.push(ostoutf8(arg));
		}
		self
	}
	pub fn sudo(self) -> Self {
		let mut out = Self::new("sudo");
		out.args(self.into_args());
		out
	}
	pub fn ssh(self, on: impl AsRef<OsStr>) -> Self {
		let mut out = Self::new("ssh");
		out.arg(on).arg("--");
		out.arg(self.into_string());
		out
	}

	pub async fn run(self) -> Result<()> {
		let str = self.clone().into_string();
		info!("running {str}");
		let mut cmd = self.into_command();
		cmd.inherit_stdio();
		let out = cmd.spawn()?.wait_with_output().await?;
		if !out.status.success() {
			anyhow::bail!("command '{}' failed with status {}", str, out.status);
		}
		Ok(())
	}
	pub async fn run_string(self) -> Result<String> {
		let str = self.clone().into_string();
		info!("running {str}");
		let mut cmd = self.into_command();
		cmd.inherit_stdio();
		cmd.stdout(Stdio::piped());
		let out = cmd.spawn()?.wait_with_output().await?;
		if !out.status.success() {
			anyhow::bail!("command '{}' failed with status {}", str, out.status);
		}
		Ok(String::from_utf8(out.stdout)?)
	}
	pub async fn run_nix_json<T: DeserializeOwned>(self) -> Result<T> {
		let str = self.run_nix_string().await?;
		serde_json::from_str(&str).with_context(|| format!("{:?}", str))
	}

	pub async fn run_nix_string(self) -> Result<String> {
		let str = self.clone().into_string();
		let mut cmd = self.into_command();
		cmd.stdout(Stdio::piped());
		run_nix_inner(str, cmd).await.map(|v| v.unwrap())
	}
	pub async fn run_nix(self) -> Result<()> {
		let str = self.clone().into_string();
		let mut cmd = self.into_command();
		cmd.stdout(Stdio::inherit());
		run_nix_inner(str, cmd).await.map(|v| {
			assert!(v.is_none());
		})
	}
}

struct EmptyAsyncRead;
impl AsyncRead for EmptyAsyncRead {
	fn poll_read(
		self: std::pin::Pin<&mut Self>,
		_cx: &mut std::task::Context<'_>,
		_buf: &mut tokio::io::ReadBuf<'_>,
	) -> Poll<std::io::Result<()>> {
		Poll::Pending
	}
}

async fn run_nix_inner(str: String, mut cmd: Command) -> Result<Option<String>> {
	info!("running {str}");
	cmd.arg("--log-format").arg("internal-json");
	cmd.stderr(Stdio::piped());
	let mut child = cmd.spawn()?;
	let mut stderr = child.stderr.take().unwrap();
	let stdout = child.stdout.take();
	let wants_stdout = stdout.is_some();
	let mut err = FramedRead::new(&mut stderr, LinesCodec::new());
	let mut out: Box<dyn AsyncRead + Unpin> = stdout
		.map(|s| Box::new(s) as Box<dyn AsyncRead + Unpin>)
		.unwrap_or_else(|| Box::new(EmptyAsyncRead));
	let mut out = FramedRead::new(&mut out, BytesCodec::new());

	// while let Some(line) = read.next().await? {}

	let mut out_buf = if wants_stdout { Some(vec![]) } else { None };
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
								if !(msg.starts_with("\u{1b}[35;1mwarning:\u{1b}[0m Git tree '") && msg.ends_with("' is dirty"))
									&& !msg.starts_with("\u{1b}[35;1mwarning:\u{1b}[0m not writing modified lock file of flake")
									&& msg != "\u{1b}[35;1mwarning:\u{1b}[0m \u{1b}[31;1merror:\u{1b}[0m SQLite database '\u{1b}[35;1m/nix/var/nix/db/db.sqlite\u{1b}[0m' is busy" {
									if let Some(raw_msg) = raw_msg {
										info!(target: "nix", "{raw_msg}\n{msg}")
									}else {
										info!(target: "nix", "{msg}")

									}
								}
							},
							NixLog::Start { ref fields, typ, .. } if typ == 105 && !fields.is_empty() => {
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
							NixLog::Start { text, level: 0, typ: 108, .. } if text.is_empty() => {
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
							NixLog::Start { text, level: 3, typ: 111, .. } if text.starts_with("resolved derivation:  ") => {
								// CA resolved
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
					out_buf.as_mut().expect("stdout == wants_stdout").extend_from_slice(&o?);
				}
			},
			code = child.wait() => {
				let code = code?;
				if !code.success() {
					anyhow::bail!("command '{str}' failed with status {}", code);
				}
				break;
			}
		}
	}

	Ok(out_buf.map(String::from_utf8).transpose()?)
}

#[async_trait]
pub trait CommandExt {
	// async fn run_nix(&mut self) -> Result<()>;
	// async fn run_nix_json<T: DeserializeOwned>(&mut self) -> Result<T>;
	// async fn run_nix_string(&mut self) -> Result<String>;
	// async fn run(&mut self) -> Result<()>;
	// async fn run_json<T: DeserializeOwned>(&mut self) -> Result<T>;
	// async fn run_string(&mut self) -> Result<String>;
	fn inherit_stdio(&mut self) -> &mut Self;
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
	fn inherit_stdio(&mut self) -> &mut Self {
		self.stderr(Stdio::inherit());
		self.stdout(Stdio::inherit());
		self
	}
}
