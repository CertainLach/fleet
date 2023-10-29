//! Calling nix eval for everything is slow, it is not easy to link nix evaluator itself,
//! and tvix-nix doesn't have proper flake support. Fleets solution: automating nix repl calls.
//!
//! Api is synchronous, yet it is good enough with pooling, and in environment without IFDs for using
//! those blocking calls from async code.

use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, bail, ensure, Context, Result};
use itertools::Itertools;
use r2d2::PooledConnection;
use rexpect::session::{PtyReplSession, PtySession};
use serde::de::DeserializeOwned;
use std::ffi::OsString;
use tracing::info_span;

fn parse_error(res: &str) -> Option<String> {
	let res = if let Some(v) = res.strip_prefix("error: ") {
		if let Some((first_line, next)) = v.split_once('\n') {
			format!("{first_line}\n{}", unindent::unindent(next))
		} else {
			v.trim_start().to_owned()
		}
	} else if let Some(v) = res.strip_prefix("error:\n") {
		let mut v = v.to_owned();
		v.insert(0, '\n');
		unindent::unindent(&v).trim_start().to_owned()
	} else {
		return None;
	};
	let res = res.trim_end();
	Some(
		res.replace('Â', "")
			.split('\n')
			.map(|l| l.strip_prefix("â\u{80}¦ ").unwrap_or(l))
			.join("\n"),
	)
}
pub struct NixSessionPool {
	pub flake: OsString,
	pub nix_args: Vec<OsString>,
}

#[derive(Debug)]
pub struct NixPoolError(anyhow::Error);
impl From<anyhow::Error> for NixPoolError {
	fn from(value: anyhow::Error) -> Self {
		Self(value)
	}
}
impl std::error::Error for NixPoolError {}
impl std::fmt::Display for NixPoolError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(f)
	}
}

impl r2d2::ManageConnection for NixSessionPool {
	type Connection = NixSession;
	type Error = NixPoolError;

	fn connect(&self) -> std::result::Result<Self::Connection, Self::Error> {
		Ok(NixSession::new(&self.flake, &self.nix_args, None)?)
	}

	fn is_valid(&self, conn: &mut Self::Connection) -> std::result::Result<(), Self::Error> {
		let res = conn.expression_result("2 + 2")?;
		if res != "4" {
			return Err(anyhow!("basic expression failed").into());
		}
		Ok(())
	}

	fn has_broken(&self, conn: &mut Self::Connection) -> bool {
		conn.finished
	}
}

pub struct NixSession {
	session: PtyReplSession,
	next_id: u32,
	free_list: Vec<u32>,
	finished: bool,
}
impl NixSession {
	fn new(flake: &OsString, args: &[OsString], timeout: Option<u64>) -> Result<Self> {
		let mut cmd = std::process::Command::new("nix");
		cmd.arg("repl");
		cmd.arg(flake);
		for arg in args {
			cmd.arg(arg);
		}
		cmd.env("TERM", "dumb");
		cmd.env("NO_COLOR", "1");
		let pty_session = rexpect::session::spawn_command(cmd, timeout)?;
		let mut repl = PtyReplSession {
			prompt: "nix-repl> ".to_string(),
			pty_session,
			quit_command: Some(":q".to_string()),
			echo_on: true,
		};
		repl.wait_for_prompt()?;
		Ok(Self {
			session: repl,
			next_id: 0,
			free_list: vec![],
			finished: false,
		})
	}
	fn expression_result(&mut self, cmd: &str) -> Result<String> {
		dbg!(cmd);
		self.session.send_line(cmd)?;
		dbg!("waiting");
		let result = self.session.wait_for_prompt()?;
		let result = strip_ansi_escapes::strip_str(&result);
		let result = result.trim();
		dbg!(result);
		Ok(result.to_owned())
	}
	fn json_result<V: DeserializeOwned>(&mut self, cmd: &str) -> Result<V> {
		let v = match self.expression_result(&format!("builtins.toJSON ({cmd})")) {
			Ok(v) => {
				if let Some(e) = parse_error(&v) {
					bail!("{e}")
				}
				v
			}
			Err(e) => {
				self.finished = true;
				bail!("{e}")
			}
		};
		// Remove outer quoting
		let v: String = serde_json::from_str(&v)?;
		Ok(serde_json::from_str(&v)?)
	}
	/// Id should be immediately used
	fn allocate_id(&mut self) -> u32 {
		if let Some(free) = self.free_list.pop() {
			free
		} else {
			let v = self.next_id;
			self.next_id += 1;
			v
		}
	}
	fn allocate_result(&mut self, cmd: &str) -> Result<u32> {
		let id = self.allocate_id();
		match self.expression_result(&format!("sess_field_{id} = ({cmd})")) {
			Ok(v) => {
				if let Some(e) = parse_error(&v) {
					self.free_list.push(id);
					bail!("{e}")
				}
			}
			Err(e) => {
				self.finished = true;
			}
		}

		Ok(id)
	}
	/// Nix has no way to deallocate variable, yet GC will correct everything not reachable.
	fn free_id(&mut self, id: u32) {
		if let Err(e) = self.expression_result(&format!("sess_field_{id} = null")) {
			self.finished = true;
		} else {
			self.free_list.push(id)
		}
	}
}

#[derive(Clone, Debug)]
enum Index {
	String(String),
	Idx(u32),
}

pub struct Field {
	full_path: Vec<Index>,
	session: Arc<Mutex<PooledConnection<NixSessionPool>>>,
	value: Option<u32>,
}
impl Field {
	pub fn root(conn: PooledConnection<NixSessionPool>) -> Self {
		Self {
			full_path: vec![],
			session: Arc::new(Mutex::new(conn)),
			value: None,
		}
	}
	pub fn get_field_deep<'a>(&self, name: impl IntoIterator<Item = &'a str>) -> Result<Self> {
		let mut iter = name.into_iter();

		let mut full_path = self.full_path.clone();
		let mut query = if let Some(id) = self.value {
			format!("sess_field_{id}")
		} else {
			let first = iter.next().expect("name not empty");
			ensure!(
				!(first.contains('.') | first.contains(' ')),
				"bad name for root query: {first}"
			);
			full_path.push(Index::String(first.to_string()));
			first.to_string()
		};
		for v in iter {
			full_path.push(Index::String(v.to_string()));
			// Escape
			let escaped = nixlike::serialize(v)?;
			let escaped = escaped.trim();
			query.push('.');
			query.push_str(escaped);
		}

		let vid = self
			.session
			.lock()
			.unwrap()
			.allocate_result(&query)
			.with_context(|| format!("full path: {:?}", full_path))?;
		Ok(Self {
			full_path,
			session: self.session.clone(),
			value: Some(vid),
		})
	}
	pub fn get_field<'a>(&self, name: &str) -> Result<Self> {
		self.get_field_deep([name])
	}
	pub fn as_json<V: DeserializeOwned>(&self) -> Result<V> {
		let id = self.value.expect("can't serialize root field");
		self.session
			.lock()
			.unwrap()
			.json_result(&format!("sess_field_{id}"))
			.with_context(|| format!("full path: {:?}", self.full_path))
	}
	pub fn list_fields(&self) -> Result<Vec<String>> {
		let id = self.value.expect("can't list root fields");
		self.session
			.lock()
			.unwrap()
			.json_result(&format!("builtins.attrNames sess_field_{id}"))
			.with_context(|| format!("full path: {:?}", self.full_path))
	}
}
impl Drop for Field {
	fn drop(&mut self) {
		if let Some(id) = self.value {
			self.session.lock().unwrap().free_id(id)
		}
	}
}
