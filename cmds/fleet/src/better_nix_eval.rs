use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, bail, ensure, Context, Result};
use futures::StreamExt;
use itertools::Itertools;
use r2d2::{Pool, PooledConnection};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::{debug, error, warn, Level};

use crate::command::{ClonableHandler, Handler, NixHandler, NoopHandler};

const REPL_DELIMITER: &str = "\"FLEET_MAGIC_REPL_DELIMITER\"";

pub struct NixSessionInner {
	full_delimiter: String,
	nix_handler: ClonableHandler<NixHandler>,
	out: OutputHandler,
	stdin: ChildStdin,
	string_wrapping: (String, String),
	number_wrapping: (String, String),

	next_id: u32,
	free_list: Vec<u32>,
}
const TRAIN_STRING: &str = "\"TRAIN_STRING\"";
const TRAIN_NUMBER: &str = "13141516";

#[must_use]
struct ErrorCollector<'i, H> {
	collected: Vec<String>,
	inner: &'i mut H,
}
impl<'i, H> ErrorCollector<'i, H> {
	fn new(inner: &'i mut H) -> Self {
		Self {
			collected: vec![],
			inner,
		}
	}
}
impl<H> ErrorCollector<'_, H> {
	fn handle_line_inner(&mut self, msg: &str) -> bool {
		let Some(msg) = msg.strip_prefix("@nix ") else {
			return false;
		};
		#[derive(Deserialize)]
		struct ErrorAction {
			action: String,
			level: u32,
			msg: String,
		}
		let Ok(act) = serde_json::from_str::<ErrorAction>(msg) else {
			return false;
		};
		if act.action != "msg" || act.level != 0 {
			return false;
		}
		self.collected.push(act.msg);
		true
	}
	fn finish(self) -> Result<()> {
		// fn dedent(s: String) -> String {
		// 	s.split('\n').filter(|s| !s.trim().is_empty()).map(|v| v.)
		// }
		if !self.collected.is_empty() {
			bail!(
				"{}",
				self.collected
					.iter()
					.map(|v| {
						if let Some(f) = v.strip_prefix("\u{1b}[31;1merror:\u{1b}[0m ") {
							let v = unindent::unindent(f.trim_start());
							v.trim().to_owned()
						} else {
							v.to_owned()
						}
					})
					.join("\n")
			);
		}
		Ok(())
	}
	fn flush(self) {
		for line in self.collected {
			warn!("{line}");
		}
	}
}
impl<H: Handler> Handler for ErrorCollector<'_, H> {
	fn handle_line(&mut self, e: &str) {
		if self.handle_line_inner(e) {
			return;
		}
		self.inner.handle_line(e)
	}
}

enum OutputLine {
	Out(String),
	Err(String),
}
struct OutputHandler {
	rx: mpsc::Receiver<OutputLine>,
	_cancel_handle: oneshot::Receiver<()>,
}
impl OutputHandler {
	fn new(out: ChildStdout, err: ChildStderr) -> Self {
		let mut out = FramedRead::new(out, LinesCodec::new());
		let mut err = FramedRead::new(err, LinesCodec::new());
		let (tx, rx) = mpsc::channel(20);
		let (mut cancelled, _cancel_handle) = oneshot::channel();
		tokio::spawn(async move {
			loop {
				select! {
					// We should receive errors earlier than synchronization
					biased;
					e = err.next() => {
						let Some(Ok(e)) = e else {
							if e.is_some() {
								error!("bad repl stderr: {e:?}");
							}
							continue;
						};
						let _ = tx.send(OutputLine::Err(e)).await;
					}
					o = out.next() => {
						let Some(Ok(o)) = o else {
							if o.is_some() {
								error!("bad repl stdout: {o:?}");
							}
							continue;
						};
						let _ = tx.send(OutputLine::Out(o)).await;
					}
					// Reader doesn't care about stdout, as this is cancelled.
					// Error still might be useful, to process leftover span closures?
					_ = cancelled.closed() => {
						break;
					}
				}
			}
		});
		Self { rx, _cancel_handle }
	}
	async fn next(&mut self) -> Option<OutputLine> {
		self.rx.recv().await
	}
}

struct WarnHandler;
impl Handler for WarnHandler {
	fn handle_line(&mut self, e: &str) {
		warn!(target: "nix", "{e}")
	}
}

impl NixSessionInner {
	async fn new(flake: &OsStr, extra_args: impl IntoIterator<Item = &OsStr>) -> Result<Self> {
		let mut cmd = Command::new("nix");
		cmd.arg("repl")
			.arg(flake)
			.arg("--log-format")
			.arg("internal-json");
		for arg in extra_args {
			cmd.arg(arg);
		}
		cmd.stdin(Stdio::piped());
		cmd.stdout(Stdio::piped());
		cmd.stderr(Stdio::piped());
		let cmd = cmd.spawn()?;
		let stdout = cmd.stdout.unwrap();
		let stderr = cmd.stderr.unwrap();
		let mut out = OutputHandler::new(stdout, stderr);
		let mut stdin = cmd.stdin.unwrap();
		// Standard repl hello doesn't work with internal-json logger
		stdin.write_all(REPL_DELIMITER.as_bytes()).await?;
		stdin.write_all(b"\n").await?;
		stdin.flush().await?;
		let nix_handler = NixHandler::default();
		let mut full_delimiter = None;
		let mut errors = vec![];
		while let Some(line) = out.next().await {
			let line = match line {
				OutputLine::Out(o) => o,
				OutputLine::Err(_e) => {
					// Handle startup errors, but skip repl hello?
					errors.push(_e);
					continue;
				}
			};
			if line.contains(REPL_DELIMITER) {
				debug!("discovered repl delimiter with added colors: {line}");
				full_delimiter = Some(line.to_owned());
				break;
			}
		}
		let Some(full_delimiter) = full_delimiter else {
			for e in errors {
				error!("{e}");
			}
			bail!("failed to discover delimiter");
		};
		let mut res = Self {
			full_delimiter,
			nix_handler: ClonableHandler::new(nix_handler),
			out,
			stdin,
			string_wrapping: Default::default(),
			number_wrapping: Default::default(),

			next_id: 0,
			free_list: vec![],
		};
		res.train().await?;
		Ok(res)
	}
	async fn train(&mut self) -> Result<()> {
		{
			let full_string = self
				.execute_expression_raw(TRAIN_STRING, &mut NoopHandler)
				.await?;
			let string_offset = full_string.find(TRAIN_STRING).expect("contained");
			let string_prefix = &full_string[..string_offset];
			let string_suffix = &full_string[string_offset + TRAIN_STRING.len()..];
			self.string_wrapping = (string_prefix.to_owned(), string_suffix.to_owned());
		}
		{
			let full_number = self
				.execute_expression_raw(TRAIN_NUMBER, &mut NoopHandler)
				.await?;
			let number_offset = full_number.find(TRAIN_NUMBER).expect("contained");
			let number_prefix = &full_number[..number_offset];
			let number_suffix = &full_number[number_offset + TRAIN_NUMBER.len()..];
			self.number_wrapping = (number_prefix.to_owned(), number_suffix.to_owned());
		}
		Ok(())
	}
	async fn send_command(&mut self, cmd: impl AsRef<[u8]>) -> Result<()> {
		if tracing::enabled!(Level::DEBUG) {
			let cmd_str = String::from_utf8_lossy(cmd.as_ref());
			tracing::debug!("{cmd_str}");
		};
		self.stdin.write_all(cmd.as_ref()).await?;
		self.stdin.write_all(b"\n").await?;
		Ok(())
	}
	async fn read_until_delimiter(&mut self, err_handler: &mut dyn Handler) -> Result<String> {
		let mut out = String::new();
		while let Some(line) = self.out.next().await {
			let line = match line {
				OutputLine::Out(out) => out,
				OutputLine::Err(err) => {
					err_handler.handle_line(&err);
					continue;
				}
			};
			if line == self.full_delimiter {
				return Ok(out);
			}
			if !out.is_empty() {
				out.push('\n');
			}
			out.push_str(&line);
		}
		bail!("didn't reached delimiter");
	}
	async fn execute_expression_number(&mut self, expr: impl AsRef<[u8]>) -> Result<u64> {
		let num = self.number_wrapping.clone();
		let n = self.execute_expression_wrapping(expr, &num).await?;
		Ok(n.parse::<u64>()?)
	}
	async fn execute_expression_string(&mut self, expr: impl AsRef<[u8]>) -> Result<String> {
		let num = self.string_wrapping.clone();
		let n = self.execute_expression_wrapping(expr, &num).await?;
		let str: String = serde_json::from_str(&n)?;
		Ok(str)
	}
	async fn execute_expression_to_json<V: DeserializeOwned>(
		&mut self,
		expr: impl AsRef<[u8]>,
	) -> Result<V> {
		let mut fexpr = b"builtins.toJSON (".to_vec();
		fexpr.extend_from_slice(expr.as_ref());
		fexpr.push(b')');
		let v = self.execute_expression_string(fexpr).await?;
		Ok(serde_json::from_str(&v)?)
	}
	async fn execute_expression_wrapping(
		&mut self,
		expr: impl AsRef<[u8]>,
		wrapping: &(String, String),
	) -> Result<String> {
		let mut nix_handler = self.nix_handler.clone();
		let mut collected = ErrorCollector::new(&mut nix_handler);
		let res = self.execute_expression_raw(expr, &mut collected).await?;
		if res.is_empty() {
			collected.finish()?;
			bail!("expected expression, got nothing")
		} else {
			collected.flush()
		};
		let Some(res) = res.strip_prefix(&wrapping.0) else {
			bail!("invalid type")
		};
		let Some(res) = res.strip_suffix(&wrapping.1) else {
			bail!("invalid type")
		};
		Ok(res.to_owned())
	}
	async fn execute_expression_empty(&mut self, expr: impl AsRef<[u8]>) -> Result<()> {
		let mut nix_handler = self.nix_handler.clone();
		let mut collected = ErrorCollector::new(&mut nix_handler);
		let v = self.execute_expression_raw(expr, &mut collected).await?;
		collected.finish()?;
		ensure!(v.is_empty(), "unexpected expression result");
		Ok(())
	}
	async fn execute_expression_raw(
		&mut self,
		expr: impl AsRef<[u8]>,
		err_handler: &mut dyn Handler,
	) -> Result<String> {
		self.send_command(expr).await?;
		// It will be echoed
		self.send_command(REPL_DELIMITER).await?;
		self.read_until_delimiter(err_handler).await
	}
	async fn execute_assign(&mut self, expr: impl AsRef<str>) -> Result<u32> {
		let id = self.allocate_id();
		self.execute_expression_empty(format!("sess_field_{id} = {}", expr.as_ref()))
			.await?;
		Ok(id)
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
	// Nix has no way to deallocate variable, yet GC will correct everything not reachable.
	// async fn free_id(&mut self, id: u32) -> Result<()> {
	// 	self.execute_expression_empty(format!("sess_field_{id} = null"))
	// 		.await?;
	// 	self.free_list.push(id);
	// 	Ok(())
	// }
}

#[derive(Clone)]
pub struct NixSession(Arc<tokio::sync::Mutex<PooledConnection<NixSessionPoolInner>>>);

#[derive(Clone)]
pub struct NixExprBuilder {
	out: String,
	used_fields: Vec<Field>,
}
impl NixExprBuilder {
	pub fn object() -> Self {
		NixExprBuilder {
			out: "{ ".to_owned(),
			used_fields: Vec::new(),
		}
	}
	pub fn string(s: &str) -> Self {
		NixExprBuilder {
			out: nixlike::serialize(s)
				.expect("no problems with serializing_string")
				.trim_end()
				.to_owned(),
			used_fields: Vec::new(),
		}
	}
	pub fn serialized(v: impl Serialize) -> Self {
		let serialized = nixlike::serialize(v).expect("invalid value for apply");
		Self {
			out: serialized.trim_end().to_owned(),
			used_fields: Vec::new(),
		}
	}
	pub fn field(f: Field) -> Self {
		Self {
			out: format!("sess_field_{}", f.0.value.expect("no value")),
			used_fields: vec![f],
		}
	}
	pub fn end_obj(&mut self) {
		self.out.push('}');
	}
	pub fn obj_key(&mut self, name: Self, value: Self) {
		self.out.push_str(r#""${"#);
		self.extend(name);
		self.out.push_str(r#"}" = "#);
		self.extend(value);
		self.out.push_str("; ");
	}

	pub fn extend(&mut self, e: Self) {
		self.out.push_str(&e.out);
		self.used_fields.extend(e.used_fields);
	}

	pub fn session(&self) -> NixSession {
		let mut session = None;
		for ele in &self.used_fields {
			if session.is_none() {
				session = Some(ele.0.session.clone());
				continue;
			}
			let session = &session.as_ref().expect("checked").0;
			let ele_sess = &ele.0.session.0;
			assert!(
				Arc::ptr_eq(session, ele_sess),
				"can't mix fields from different session"
			);
		}
		session.expect("expr without fields used")
	}
	pub fn index_attr(&mut self, s: &str) {
		let escaped = nixlike::serialize(s).expect("string");
		self.out.push('.');
		self.out.push_str(escaped.trim_end());
	}
}

#[macro_export]
macro_rules! nix_expr_inner {
	(Obj { $($ident:ident: $($val:tt)+),* $(,)? }) => {{
		use $crate::better_nix_eval::NixExprBuilder;
		let mut out = NixExprBuilder::object();
		$(
			out.obj_key(
				NixExprBuilder::string(stringify!($ident)),
				$crate::nix_expr_inner!($($val)+),
			);
		)*
		out.end_obj();
		out
	}};
	(@field($o:ident) . $var:ident $($tt:tt)*) => {{
		$o.index_attr(stringify!($var));
		nix_expr_inner!(@field($o) $($tt)*);
	}};
	(@field($o:ident) [{ $v:expr }] $($tt:tt)*) => {{
		$o.push(Index::attr(&$v));
		nix_expr_inner!(@o($o) $($tt)*);
	}};
	(@field($o:ident) [ $($var:tt)+ ] $($tt:tt)*) => {{
		$o.push(Index::Expr($crate::nix_expr_inner!($($var)+)));
		nix_expr_inner!(@o($o) $($tt)*);
	}};
	(@field($o:ident) ($($var:tt)*) $($tt:tt)*) => {
		$o.push(Index::ExprApply($crate::nix_expr_inner!($($var)+)));
		nix_expr_inner!(@o($o) $($tt)*);
	};
	(@field($o:ident)) => {};
	($field:ident $($tt:tt)*) => {{
		use $crate::{better_nix_eval::NixExprBuilder, nix_expr_inner};
		#[allow(unused_mut, reason = "might be used if indexed")]
		let mut out = NixExprBuilder::field($field.clone());
		nix_expr_inner!(@field(out) $($tt)*);
		out
	}};
	($v:literal) => {{
		use $crate::better_nix_eval::NixExprBuilder;
		NixExprBuilder::string($v)
	}};
	({$v:expr}) => {{
		use $crate::better_nix_eval::NixExprBuilder;
		NixExprBuilder::serialized(&$v)
	}}
}
#[macro_export]
macro_rules! nix_expr {
	($($tt:tt)+) => {{
		use $crate::{better_nix_eval::{NixExprBuilder, Field}, nix_expr_inner};
		let expr = nix_expr_inner!($($tt)+);
		Field::new(expr.session(), expr.out)
	}};
}

#[macro_export]
macro_rules! nix_go {
	(@o($o:ident) . $var:ident $($tt:tt)*) => {{
		$o.push(Index::attr(stringify!($var)));
		nix_go!(@o($o) $($tt)*);
	}};
	(@o($o:ident) [{ $v:expr }] $($tt:tt)*) => {{
		$o.push(Index::attr(&$v));
		nix_go!(@o($o) $($tt)*);
	}};
	(@o($o:ident) [ $($var:tt)+ ] $($tt:tt)*) => {{
		$o.push(Index::Expr($crate::nix_expr_inner!($($var)+)));
		nix_go!(@o($o) $($tt)*);
	}};
	(@o($o:ident) ($($var:tt)*) $($tt:tt)*) => {
		$o.push(Index::ExprApply($crate::nix_expr_inner!($($var)+)));
		nix_go!(@o($o) $($tt)*);
	};
	(@o($o:ident)) => {};
	($field:ident $($tt:tt)+) => {{
		use $crate::{nix_go, better_nix_eval::Index};
		let field = $field.clone();
		let mut out = vec![];
		nix_go!(@o(out) $($tt)*);
		field.select(out).await?
	}}
}
#[macro_export]
macro_rules! nix_go_json {
	($($tt:tt)*) => {{
		$crate::nix_go!($($tt)*).as_json().await?
	}};
}

#[derive(Clone)]
pub enum Index {
	Var(String),
	String(String),
	Apply(String),
	Expr(NixExprBuilder),
	ExprApply(NixExprBuilder),
}
impl Index {
	pub fn var(v: impl AsRef<str>) -> Self {
		let v = v.as_ref();
		assert!(
			!(v.contains('.') | v.contains(' ')),
			"bad variable name: {v}"
		);
		Self::Var(v.to_owned())
	}
	pub fn attr(v: impl AsRef<str>) -> Self {
		Self::String(v.as_ref().to_owned())
	}
	pub fn apply(v: impl Serialize) -> Self {
		let serialized = nixlike::serialize(v).expect("invalid value for apply");
		Self::Apply(serialized.trim_end().to_owned())
	}
}
impl Display for Index {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Index::Var(v) => {
				write!(f, "{v}")
			}
			Index::String(k) => {
				let v = nixlike::format_identifier(k.as_str());
				write!(f, ".{v}")
			}
			Index::Apply(o) => {
				write!(f, "<apply>({o})")
			}
			Index::Expr(e) => {
				write!(f, "[{}]", e.out)
			}
			Index::ExprApply(e) => {
				write!(f, "<apply>({})", e.out)
			}
		}
	}
}
impl fmt::Debug for Index {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{self}")
	}
}
struct PathDisplay<'i>(&'i [Index]);
impl Display for PathDisplay<'_> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		for i in self.0 {
			write!(f, "{i}")?;
		}
		Ok(())
	}
}
struct FieldInner {
	full_path: Option<Vec<Index>>,
	session: NixSession,
	value: Option<u32>,
}
fn context(full_path: Option<&[Index]>, query: &str) -> String {
	if let Some(full_path) = &full_path {
		format!("full path: {}", PathDisplay(full_path))
	} else {
		format!("query: {query:?}")
	}
}
#[derive(Clone)]
pub struct Field(Arc<FieldInner>);
impl Field {
	fn root(session: NixSession) -> Self {
		Self(Arc::new(FieldInner {
			full_path: Some(vec![]),
			session,
			value: None,
		}))
	}
	async fn new(session: NixSession, query: &str) -> Result<Self> {
		let vid = session
			.0
			.lock()
			.await
			.execute_assign(query)
			.await
			.with_context(|| context(None, query))?;
		Ok(Self(Arc::new(FieldInner {
			full_path: None,
			session,
			value: Some(vid),
		})))
	}
	pub async fn field(session: NixSession, field: &str) -> Result<Self> {
		Self::root(session).select([Index::var(field)]).await
	}
	pub async fn get_json_deep<'a, V: DeserializeOwned>(
		&self,
		name: impl IntoIterator<Item = Index>,
	) -> Result<V> {
		let field = self.select(name).await?;
		field.as_json().await
	}
	pub async fn select<'a>(&self, name: impl IntoIterator<Item = Index>) -> Result<Self> {
		let mut used_fields = Vec::new();
		let mut name = name.into_iter();

		let mut full_path = self.0.full_path.clone();
		let mut query = if let Some(id) = self.0.value {
			format!("sess_field_{id}")
		} else {
			let first = name.next();
			if let Some(Index::Var(i)) = first {
				if let Some(full_path) = &mut full_path {
					full_path.push(Index::Var(i.clone()));
				}
				i.clone()
			} else {
				panic!("first path item should be variable, got {first:?}")
			}
		};
		for v in name {
			if let Some(full_path) = &mut full_path {
				full_path.push(v.clone());
			}
			match v {
				Index::Var(_) => panic!("var item may only be first"),
				Index::String(s) => {
					let escaped = nixlike::serialize(s)?;
					query.push('.');
					query.push_str(escaped.trim());
				}
				Index::Apply(a) => {
					// In cases like `a {}.b` first `{}.b` will be evaluated, so `a {}` should be encased in `()`
					query = format!("({query} {a})");
				}
				Index::Expr(e) => {
					let index = Field::new(self.0.session.clone(), &e.out).await?;
					used_fields.push(index.clone());
					query.push('.');
					let index = format!("${{sess_field_{}}}", index.0.value.expect("value"));
					query.push_str(&index);
				}
				Index::ExprApply(e) => {
					let index = Field::new(self.0.session.clone(), &e.out).await?;
					used_fields.push(index.clone());
					query.push(' ');
					let index = format!("sess_field_{}", index.0.value.expect("value"));
					query.push_str(&index);
					query = format!("({query})");
				}
			}
		}

		let vid = self
			.0
			.session
			.0
			.lock()
			.await
			.execute_assign(&query)
			.await
			.with_context(|| {
				if let Some(full_path) = &full_path {
					format!("full path: {}", PathDisplay(full_path))
				} else {
					format!("query: {query:?}")
				}
			})?;
		Ok(Self(Arc::new(FieldInner {
			full_path,
			session: self.0.session.clone(),
			value: Some(vid),
		})))
	}
	pub async fn as_json<V: DeserializeOwned>(&self) -> Result<V> {
		let id = self.0.value.expect("can't serialize root field");
		let query = format!("sess_field_{id}");
		self.0
			.session
			.0
			.lock()
			.await
			.execute_expression_to_json(&query)
			.await
			.with_context(|| context(self.0.full_path.as_deref(), &query))
	}
	pub async fn list_fields(&self) -> Result<Vec<String>> {
		let id = self.0.value.expect("can't list root fields");
		let query = format!("builtins.attrNames sess_field_{id}");
		self.0
			.session
			.0
			.lock()
			.await
			.execute_expression_to_json(&query)
			.await
			.with_context(|| context(self.0.full_path.as_deref(), &query))
	}
	pub async fn build(&self) -> Result<HashMap<String, PathBuf>> {
		let id = self.0.value.expect("can't use build on not-value");
		let query = format!(":b sess_field_{id}");
		let vid = self
			.0
			.session
			.0
			.lock()
			.await
			.execute_expression_raw(&query, &mut NixHandler::default())
			.await?;
		ensure!(
			!vid.is_empty(),
			"build failed: {}",
			context(self.0.full_path.as_deref(), &query),
		);
		let Some(vid) = vid.strip_prefix("This derivation produced the following outputs:\n")
		else {
			panic!("unexpected build output: {vid:?}");
		};
		let outputs = vid
			.split('\n')
			.filter(|v| !v.is_empty())
			.map(|v| v.split_once(" -> ").expect("unexpected build output"))
			.map(|(a, b)| (a.trim_start().to_owned(), PathBuf::from(b)))
			.collect();
		Ok(outputs)
	}
}
impl Drop for FieldInner {
	fn drop(&mut self) {
		if let Some(id) = self.value {
			if let Ok(mut lock) = self.session.0.try_lock() {
				lock.free_list.push(id)
			}
			// Leaked
		}
	}
}
struct NixSessionPoolInner {
	flake: OsString,
	nix_args: Vec<OsString>,
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
impl r2d2::ManageConnection for NixSessionPoolInner {
	type Connection = NixSessionInner;
	type Error = NixPoolError;
	fn connect(&self) -> std::result::Result<Self::Connection, Self::Error> {
		let _v = TOKIO_RUNTIME
			.get()
			.expect("missed tokio runtime init!")
			.enter();
		Ok(futures::executor::block_on(NixSessionInner::new(
			self.flake.as_os_str(),
			self.nix_args.iter().map(OsString::as_os_str),
		))?)
	}

	fn is_valid(&self, conn: &mut Self::Connection) -> std::result::Result<(), Self::Error> {
		let _v = TOKIO_RUNTIME
			.get()
			.expect("missed tokio runtime init!")
			.enter();
		let res = futures::executor::block_on(conn.execute_expression_number("2 + 2"))?;
		if res != 4 {
			return Err(anyhow!("sanity check failed").into());
		};
		Ok(())
	}

	fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
		false
	}
}
pub struct NixSessionPool(Pool<NixSessionPoolInner>);
impl NixSessionPool {
	pub async fn new(flake: OsString, nix_args: Vec<OsString>) -> Result<Self> {
		let inner = tokio::task::block_in_place(|| {
			r2d2::Builder::<NixSessionPoolInner>::new()
				.min_idle(Some(0))
				.build(NixSessionPoolInner { flake, nix_args })
		})?;
		Ok(Self(inner))
	}
	pub async fn get(&self) -> Result<NixSession> {
		let v = tokio::task::block_in_place(|| self.0.get())?;
		Ok(NixSession(Arc::new(tokio::sync::Mutex::new(v))))
	}
}

pub static TOKIO_RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();
