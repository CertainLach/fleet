use std::{
	collections::HashMap,
	ffi::OsStr,
	pin,
	process::Stdio,
	sync::{Arc, Mutex},
	task::Poll,
};

use anyhow::{anyhow, Result};
use futures::StreamExt;
use itertools::Either;
use once_cell::sync::Lazy;
use openssh::{OverSsh, OwningCommand, Session};
use regex::Regex;
use serde::{de::Visitor, Deserialize};
use tokio::{io::AsyncRead, process::Command, select};
use tokio_util::codec::{BytesCodec, FramedRead, LinesCodec};
use tracing::{info, info_span, warn, Span};
use tracing_indicatif::span_ext::IndicatifSpanExt;

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
	ssh_session: Option<Arc<Session>>,
}
impl MyCommand {
	pub fn new_on(cmd: impl AsRef<OsStr>, session: Arc<Session>) -> Self {
		assert!(!cmd.as_ref().is_empty());
		Self {
			command: ostoutf8(cmd),
			args: vec![],
			env: vec![],
			ssh_session: Some(session),
		}
	}
	pub fn new(cmd: impl AsRef<OsStr>) -> Self {
		assert!(!cmd.as_ref().is_empty());
		Self {
			command: ostoutf8(cmd),
			args: vec![],
			env: vec![],
			ssh_session: None,
		}
	}
	fn into_args(self) -> Vec<String> {
		let mut out = Vec::new();
		if !self.env.is_empty() {
			out.push("env".to_owned());
			for (k, v) in self.env {
				assert!(!k.contains('='));
				out.push(format!("{k}={v}"));
			}
		}
		out.push(self.command);
		out.extend(self.args);
		out
	}

	/// Translates environment variables into env command execution.
	/// Required for ssh, as ssh don't allow to send environment variables (at least by default).
	///
	/// FIXME: Insecure, as arguments might be seen by other users on the same machine.
	/// Figure out some way to transfer environment using stdio?
	fn translate_env_into_env(self) -> Self {
		if self.env.is_empty() {
			return self;
		}
		let mut out = Self::new("env");
		if let Some(session) = self.ssh_session {
			out = out.ssh_session(session);
		}
		for (k, v) in self.env {
			assert!(!k.contains('='));
			out.arg(format!("{k}={v}"));
		}
		out.arg(self.command);
		out.args(self.args);

		out
	}
	fn into_string(self) -> String {
		let mut out = String::new();
		if !self.env.is_empty() {
			out.push_str("env");
			for (k, v) in self.env {
				out.push(' ');
				assert!(!k.contains('='));
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
	fn into_command_new(self) -> Result<Either<Command, openssh::OwningCommand<Arc<Session>>>> {
		Ok(if let Some(session) = self.ssh_session.clone() {
			let cmd = self.translate_env_into_env().into_command();
			Either::Right(
				cmd.over_ssh(session)
					.map_err(|e| anyhow!("ssh error: {e}"))?,
			)
		} else {
			let cmd = self.into_command();
			Either::Left(cmd)
		})
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
	pub fn env(&mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> &mut Self {
		self.env
			.push((name.as_ref().to_owned(), value.as_ref().to_owned()));
		self
	}
	pub fn args<V: AsRef<OsStr>>(&mut self, args: impl IntoIterator<Item = V>) -> &mut Self {
		for arg in args.into_iter() {
			let arg = arg.as_ref();
			self.args.push(ostoutf8(arg));
		}
		self
	}
	pub fn sudo(mut self) -> Self {
		if std::env::var_os("NO_SUDO").is_some() {
			let mut out = Self::new("su");
			out.ssh_session = self.ssh_session.take();
			out.arg("-c").arg(self.into_string());
			out
		} else {
			let mut out = Self::new("sudo");
			out.args(self.into_args());
			out
		}
	}
	pub fn ssh_session(mut self, on: Arc<Session>) -> Self {
		self.ssh_session = Some(on);
		self
	}
	pub fn ssh(mut self, on: impl AsRef<OsStr>) -> Self {
		let mut out = Self::new("ssh");
		out.ssh_session = self.ssh_session.take();
		out.arg(on).arg("--");
		out.arg(self.into_string());
		out
	}

	pub async fn run(self) -> Result<()> {
		let str = self.clone().into_string();
		let cmd = self.into_command_new()?;
		match cmd {
			Either::Left(cmd) => run_nix_inner(str, cmd, &mut PlainHandler).await?,
			Either::Right(cmd) => run_nix_inner_ssh(str, cmd, &mut PlainHandler).await?,
		};
		Ok(())
	}
	pub async fn run_string(self) -> Result<String> {
		let bytes = self.run_bytes().await?;
		Ok(String::from_utf8(bytes)?)
	}
	pub async fn run_bytes(self) -> Result<Vec<u8>> {
		let str = self.clone().into_string();
		let cmd = self.into_command_new()?;
		let v = match cmd {
			Either::Left(cmd) => run_nix_inner_stdout(str, cmd, &mut PlainHandler).await?,
			Either::Right(cmd) => run_nix_inner_stdout_ssh(str, cmd, &mut PlainHandler).await?,
		};
		Ok(v)
	}

	pub async fn run_nix_string(self) -> Result<String> {
		let str = self.clone().into_string();
		let mut cmd = self.into_command();
		cmd.arg("--log-format").arg("internal-json");
		let bytes = run_nix_inner_stdout(str, cmd, &mut NixHandler::default()).await?;
		Ok(String::from_utf8(bytes)?)
	}
	pub async fn run_nix(self) -> Result<()> {
		let str = self.clone().into_string();
		let mut cmd = self.into_command();
		cmd.arg("--log-format").arg("internal-json");
		cmd.stdout(Stdio::inherit());
		run_nix_inner(str, cmd, &mut NixHandler::default()).await
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

async fn run_nix_inner_stdout(
	str: String,
	cmd: Command,
	handler: &mut dyn Handler,
) -> Result<Vec<u8>> {
	Ok(run_nix_inner_raw(str, cmd, true, handler, None)
		.await?
		.expect("has out"))
}
async fn run_nix_inner(str: String, cmd: Command, handler: &mut dyn Handler) -> Result<()> {
	let v = run_nix_inner_raw(str, cmd, false, handler, None).await?;
	assert!(v.is_none());
	Ok(())
}
async fn run_nix_inner_stdout_ssh(
	str: String,
	cmd: OwningCommand<Arc<Session>>,
	handler: &mut dyn Handler,
) -> Result<Vec<u8>> {
	Ok(run_nix_inner_raw_ssh(str, cmd, true, handler, None)
		.await?
		.expect("has out"))
}
async fn run_nix_inner_ssh(
	str: String,
	cmd: OwningCommand<Arc<Session>>,
	handler: &mut dyn Handler,
) -> Result<()> {
	let v = run_nix_inner_raw_ssh(str, cmd, false, handler, None).await?;
	assert!(v.is_none());
	Ok(())
}

pub trait Handler: Send {
	fn handle_line(&mut self, e: &str);
}

pub struct ClonableHandler<H>(Arc<Mutex<H>>);
impl<H> Clone for ClonableHandler<H> {
	fn clone(&self) -> Self {
		Self(self.0.clone())
	}
}
impl<H> ClonableHandler<H> {
	pub fn new(inner: H) -> Self {
		Self(Arc::new(Mutex::new(inner)))
	}
}
impl<H: Handler> Handler for ClonableHandler<H> {
	fn handle_line(&mut self, e: &str) {
		self.0.lock().unwrap().handle_line(e)
	}
}

struct PlainHandler;
impl Handler for PlainHandler {
	fn handle_line(&mut self, e: &str) {
		info!(target: "log", "{e}");
	}
}

pub struct NoopHandler;
impl Handler for NoopHandler {
	fn handle_line(&mut self, _e: &str) {}
}

#[derive(Default)]
pub struct NixHandler {
	spans: HashMap<u64, Span>,
}
fn process_message(m: &str) -> String {
	static OSC_CLEANER: Lazy<Regex> =
		Lazy::new(|| Regex::new(r"\x1B\]([^\x07\x1C]*[\x07\x1C])?|\r").unwrap());
	static DETABBER: Lazy<Regex> = Lazy::new(|| Regex::new(r"\t").unwrap());
	let m = OSC_CLEANER.replace_all(m, "");
	// Indicatif can't format tabs. This is not the correct tab formatting, as correct one should be aligned,
	// and not just be replaced with the constant number of spaces, but it's ok for now, as statuses are single-line.
	DETABBER.replace_all(m.as_ref(), "  ").to_string()
}
impl Handler for NixHandler {
	fn handle_line(&mut self, e: &str) {
		if let Some(e) = e.strip_prefix("@nix ") {
			let log: NixLog = match serde_json::from_str(e) {
				Ok(l) => l,
				Err(err) => {
					warn!("failed to parse nix log line {:?}: {}", e, err);
					return;
				}
			};
			match log {
				NixLog::Msg { msg, raw_msg, .. } => {
					#[allow(clippy::nonminimal_bool)]
					if !(msg.starts_with("\u{1b}[35;1mwarning:\u{1b}[0m Git tree '") && msg.ends_with("' is dirty"))
					&& !msg.starts_with("\u{1b}[35;1mwarning:\u{1b}[0m not writing modified lock file of flake")
					&& msg != "\u{1b}[35;1mwarning:\u{1b}[0m \u{1b}[31;1merror:\u{1b}[0m SQLite database '\u{1b}[35;1m/nix/var/nix/db/db.sqlite\u{1b}[0m' is busy" {
						if let Some(raw_msg) = raw_msg {
							if !msg.is_empty() {
								info!(target: "nix", "{}\n{}", raw_msg.trim_end(), msg.trim_end())
							} else {
								info!(target: "nix", "{}", raw_msg.trim_end())
							}
						} else {
							info!(target: "nix", "{}", msg.trim_end())
						}
					}
				}
				NixLog::Start {
					ref fields,
					typ,
					id,
					..
				} if typ == 105 && !fields.is_empty() => {
					if let [LogField::String(drv), ..] = &fields[..] {
						let mut drv = drv.as_str();
						if let Some(pkg) = drv.strip_prefix("/nix/store/") {
							let mut it = pkg.splitn(2, '-');
							it.next();
							if let Some(pkg) = it.next() {
								drv = pkg;
							}
						}
						info!(target: "nix","building {}", drv);
						let span = info_span!("build", drv);
						span.pb_start();
						self.spans.insert(id, span);
					} else {
						warn!("bad build log: {:?}", log)
					}
				}
				NixLog::Start {
					ref fields,
					typ,
					id,
					..
				} if typ == 100 && fields.len() >= 3 => {
					if let [LogField::String(drv), LogField::String(from), LogField::String(to), ..] =
						&fields[..]
					{
						let mut drv = drv.as_str();

						if let Some(pkg) = drv.strip_prefix("/nix/store/") {
							let mut it = pkg.splitn(2, '-');
							it.next();
							if let Some(pkg) = it.next() {
								drv = pkg;
							}
						}
						// info!(target: "nix","copying {} {} -> {}", drv, from, to);
						let span = info_span!("copy", from, to, drv);
						span.pb_start();
						self.spans.insert(id, span);
					} else {
						warn!("bad copy log: {:?}", log)
					}
				}
				NixLog::Start { text, typ, id, .. }
					if typ == 0 || typ == 102 || typ == 103 || typ == 104 =>
				{
					if !text.is_empty()
						&& text != "querying info about missing paths"
						&& text != "copying 0 paths"
						// Too much spam on lazy-trees branch
						&& !(text.starts_with("copying '") && text.ends_with("' to the store"))
					{
						let span = info_span!("job");
						span.pb_start();
						span.pb_set_message(&process_message(text.trim()));
						self.spans.insert(id, span);
						info!(target: "nix", "{}", text);
					}
				}
				NixLog::Start {
					text,
					level: 0,
					typ: 108,
					..
				} if text.is_empty() => {
					// Cache lookup? Coupled with copy log
				}
				NixLog::Start {
					text,
					level: 4,
					typ: 109,
					..
				} if text.starts_with("querying info about ") => {
					// Cache lookup
				}
				NixLog::Start {
					text,
					level: 4,
					typ: 101,
					..
				} if text.starts_with("downloading ") => {
					// NAR downloading, coupled with copy log
				}
				NixLog::Start {
					text,
					level: 1,
					typ: 111,
					..
				} if text.starts_with("waiting for a machine to build ") => {
					// Useless repeating notification about build
				}
				NixLog::Start {
					text,
					level: 3,
					typ: 111,
					..
				} if text.starts_with("resolved derivation: ") => {
					// CA resolved
				}
				NixLog::Start {
					text,
					level: 1,
					typ: 111,
					id,
					..
				} if text.starts_with("waiting for lock on ") => {
					let mut drv = text.strip_prefix("waiting for lock on ").unwrap();
					if let Some(txt) = drv.strip_prefix("\u{1b}[35;1m'") {
						drv = txt;
					}
					if let Some(txt) = drv.strip_suffix("'\u{1b}[0m") {
						drv = txt;
					}
					if let Some(txt) = drv.split("', '").next() {
						drv = txt;
					}
					if let Some(pkg) = drv.strip_prefix("/nix/store/") {
						let mut it = pkg.splitn(2, '-');
						it.next();
						if let Some(pkg) = it.next() {
							drv = pkg;
						}
					}
					let span = info_span!("waiting on drv", drv);
					span.pb_start();
					self.spans.insert(id, span);
					// Concurrent build of the same message
				}
				NixLog::Stop { id, .. } => {
					self.spans.remove(&id);
				}
				NixLog::Result { fields, id, typ } if typ == 101 && !fields.is_empty() => {
					if let Some(span) = self.spans.get(&id) {
						if let LogField::String(s) = &fields[0] {
							span.pb_set_message(&process_message(s.trim()));
						} else {
							warn!("bad fields: {fields:?}");
						}
					} else {
						warn!("unknown result id: {id} {typ} {fields:?}");
					}
					// dbg!(fields, id, typ);
				}
				NixLog::Result { fields, id, typ } if typ == 105 && fields.len() >= 4 => {
					if let Some(span) = self.spans.get(&id) {
						if let [LogField::Num(done), LogField::Num(expected), LogField::Num(_running), LogField::Num(_failed)] =
							&fields[..4]
						{
							span.pb_set_length(*expected);
							span.pb_set_position(*done);
						} else {
							warn!("bad fields: {fields:?}");
						}
					} else {
						// warn!("unknown result id: {id} {typ} {fields:?}");
						// Unaccounted progress.
					}
					// dbg!(fields, id, typ);
				}
				NixLog::Result { typ, .. } if typ == 104 || typ == 106 => {
					// Set phase, expected
				}
				_ => warn!("unknown log: {:?}", log),
			};
		} else {
			let e = e.trim();
			if e.starts_with("Failed tcsetattr(TCSADRAIN): ") {
				return;
			}
			info!("{e}")
		}
	}
}

async fn run_nix_inner_raw(
	str: String,
	mut cmd: Command,
	want_stdout: bool,
	err_handler: &mut dyn Handler,
	mut out_handler: Option<&mut dyn Handler>,
) -> Result<Option<Vec<u8>>> {
	cmd.stderr(Stdio::piped());
	cmd.stdout(Stdio::piped());
	let mut child = cmd.spawn()?;
	let mut stderr = child.stderr.take().unwrap();
	let stdout = child.stdout.take().unwrap();
	let mut err = FramedRead::new(&mut stderr, LinesCodec::new());
	let mut out: Option<Box<dyn AsyncRead + Unpin>> = Some(Box::new(stdout));
	let mut ob = want_stdout
		.then(|| out.take().unwrap())
		.unwrap_or_else(|| Box::new(EmptyAsyncRead));
	let mut ol = (!want_stdout)
		.then(|| out.take().unwrap())
		.unwrap_or_else(|| Box::new(EmptyAsyncRead));
	let mut ob = FramedRead::new(&mut ob, BytesCodec::new());
	let mut ol = FramedRead::new(&mut ol, LinesCodec::new());

	// while let Some(line) = read.next().await? {}

	let mut out_buf = if want_stdout { Some(vec![]) } else { None };
	loop {
		select! {
			e = err.next() => {
				if let Some(e) = e {
					let e = e?;
					err_handler.handle_line(&e);
				}
			},
			o = ob.next() => {
				if let Some(o) = o {
					out_buf.as_mut().expect("stdout == wants_stdout").extend_from_slice(&o?);
				}
			},
			o = ol.next() => {
				if let Some(o) = o {
					let o = o?;
					if let Some(out) = out_handler.as_mut() {
						out.handle_line(&o)
					} else {
						err_handler.handle_line(&o)
					}
					// out_handler.handle_info(&o);
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

	Ok(out_buf)
}
async fn run_nix_inner_raw_ssh(
	str: String,
	mut cmd: OwningCommand<Arc<Session>>,
	want_stdout: bool,
	err_handler: &mut dyn Handler,
	mut out_handler: Option<&mut dyn Handler>,
) -> Result<Option<Vec<u8>>> {
	cmd.stderr(openssh::Stdio::piped());
	cmd.stdout(openssh::Stdio::piped());
	let mut child = cmd.spawn().await?;
	let mut stderr = child.stderr().take().unwrap();
	let stdout = child.stdout().take().unwrap();
	let mut err = FramedRead::new(&mut stderr, LinesCodec::new());
	let mut out: Option<Box<dyn AsyncRead + Unpin>> = Some(Box::new(stdout));
	let mut ob = want_stdout
		.then(|| out.take().unwrap())
		.unwrap_or_else(|| Box::new(EmptyAsyncRead));
	let mut ol = (!want_stdout)
		.then(|| out.take().unwrap())
		.unwrap_or_else(|| Box::new(EmptyAsyncRead));
	let mut ob = FramedRead::new(&mut ob, BytesCodec::new());
	let mut ol = FramedRead::new(&mut ol, LinesCodec::new());

	// while let Some(line) = read.next().await? {}

	let mut out_buf = if want_stdout { Some(vec![]) } else { None };

	let mut wait_future = pin::pin!(child.wait());
	loop {
		select! {
			e = err.next() => {
				if let Some(e) = e {
					let e = e?;
					err_handler.handle_line(&e);
				}
			},
			o = ob.next() => {
				if let Some(o) = o {
					out_buf.as_mut().expect("stdout == wants_stdout").extend_from_slice(&o?);
				}
			},
			o = ol.next() => {
				if let Some(o) = o {
					let o = o?;
					if let Some(out) = out_handler.as_mut() {
						out.handle_line(&o)
					} else {
						err_handler.handle_line(&o)
					}
					// out_handler.handle_info(&o);
				}
			},
			code = &mut wait_future => {
				let code = code?;
				if !code.success() {
					anyhow::bail!("command '{str}' failed with status {}", code);
				}
				break;
			}
		}
	}

	Ok(out_buf)
}

pub trait ErrorRecorder: Send {
	/// Return true to discard message from logging
	fn push_message(&mut self, msg: &str) -> bool;
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
		#[serde(default)]
		fields: Vec<LogField>,
	},
}
