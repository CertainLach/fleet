use std::ffi::{OsStr, OsString};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use abort_on_drop::ChildTask;
use anyhow::{anyhow, bail, ensure, Context, Result};
use futures::StreamExt;
use r2d2::{Pool, PooledConnection};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::oneshot;
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::debug;

use crate::command::{process_child_stderr, ErrorRecorder, ErrorRecorderT, NixHandler};

const REPL_DELIMITER: &str = "\"FLEET_MAGIC_REPL_DELIMITER\"";
// To synchronize stderr and stdout. It works, yet I hate this.
// There is no other way to catch errors, because they are coming from different streams, and they are not synchronized in tokio.
const ERROR_DELIMITER: &str = "FLEET_MAGIC_ERROR_DELIMITER";

pub struct NixSessionInner {
	full_delimiter: String,
	#[allow(dead_code)]
	stderr_handler: ChildTask<Result<()>>,
	error_recorder: ErrorRecorderT,
	read: FramedRead<ChildStdout, LinesCodec>,
	stdin: ChildStdin,
	string_wrapping: (String, String),
	number_wrapping: (String, String),
	error_delimiter: String,

	next_id: u32,
	free_list: Vec<u32>,
}
const TRAIN_STRING: &str = "\"TRAIN_STRING\"";
const TRAIN_NUMBER: &str = "13141516";

struct ErrorRecorderHandle {
	handle: ErrorRecorderT,
}
impl ErrorRecorderHandle {}
impl Drop for ErrorRecorderHandle {
	fn drop(&mut self) {
		let mut recorded = self.handle.lock().unwrap();
		assert!(recorded.is_some(), "exclusive");
		*recorded = None;
	}
}

struct ErrorCollector {
	collected: Arc<Mutex<Vec<String>>>,
	delim: String,
	got_delim: Option<oneshot::Sender<()>>,
}
impl ErrorRecorder for ErrorCollector {
	fn push_message(&mut self, msg: &str) -> bool {
		if msg == self.delim {
			let _ = self.got_delim
				.take()
				.expect("error delim is only expected once")
				.send(());
			 return true;
		}
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
		self.collected.lock().unwrap().push(act.msg);
		true
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
		let mut stdin = cmd.stdin.unwrap();
		let error_recorder = ErrorRecorderT::default();
		let err_recorder = error_recorder.clone();
		let stderr_handler = abort_on_drop::ChildTask::from(tokio::spawn(async move {
			let mut handler = NixHandler::default();
			process_child_stderr(stderr, &mut handler, err_recorder).await
		}));
		// Standard repl hello doesn't work with internal-json logger
		stdin.write_all(REPL_DELIMITER.as_bytes()).await?;
		stdin.write_all(b"\n").await?;
		stdin.flush().await?;
		let mut read = FramedRead::new(stdout, LinesCodec::new());
		let mut full_delimiter = None;
		while let Some(line) = read.next().await {
			let line = line?;
			if line.contains(REPL_DELIMITER) {
				debug!("discovered repl delimiter with added colors: {line}");
				full_delimiter = Some(line.to_owned());
				break;
			}
		}
		let Some(full_delimiter) = full_delimiter else {
			bail!("failed to discover delimiter");
		};
		let mut res = Self {
			full_delimiter,
			error_delimiter: "[[filled after training]]".to_owned(),
			stderr_handler,
			error_recorder,
			read,
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
			let full_string = self.execute_expression_raw(TRAIN_STRING).await?;
			let string_offset = full_string.find(TRAIN_STRING).expect("contained");
			let string_prefix = &full_string[..string_offset];
			let string_suffix = &full_string[string_offset + TRAIN_STRING.len()..];
			self.string_wrapping = (string_prefix.to_owned(), string_suffix.to_owned());
		}
		{
			let full_number = self.execute_expression_raw(TRAIN_NUMBER).await?;
			let number_offset = full_number.find(TRAIN_NUMBER).expect("contained");
			let number_prefix = &full_number[..number_offset];
			let number_suffix = &full_number[number_offset + TRAIN_NUMBER.len()..];
			self.number_wrapping = (number_prefix.to_owned(), number_suffix.to_owned());
		}
		{
			struct TrainingErrorCollector(Option<oneshot::Sender<String>>);
			impl ErrorRecorder for TrainingErrorCollector {
				fn push_message(&mut self, msg: &str) -> bool {
					if msg.contains(ERROR_DELIMITER) {
						let _ = self
							.0
							.take()
							.expect("error delimiter is sent once")
							.send(msg.to_owned());
					}
					true
				}
			}
			let (tx, rx) = oneshot::channel();
			let _handle = self.record_error(TrainingErrorCollector(Some(tx)));
			self.send_command(ERROR_DELIMITER).await?;
			self.send_command(REPL_DELIMITER).await?;
			self.read_until_delimiter().await?;
			let msg = rx.await?;
			self.error_delimiter = msg;
		}
		Ok(())
	}
	fn record_error(&mut self, v: impl ErrorRecorder + 'static) -> ErrorRecorderHandle {
		{
			let mut recorder = self.error_recorder.lock().unwrap();
			assert!(recorder.is_none(), "recorder is already started");
			*recorder = Some(Box::new(v));
		}
		ErrorRecorderHandle {
			handle: self.error_recorder.clone(),
		}
	}
	async fn send_command(&mut self, cmd: impl AsRef<[u8]>) -> Result<()> {
		self.stdin.write_all(cmd.as_ref()).await?;
		self.stdin.write_all(b"\n").await?;
		Ok(())
	}
	async fn read_until_delimiter(&mut self) -> Result<String> {
		let mut out = String::new();
		while let Some(line) = self.read.next().await {
			let line = line?;
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
		let collected = Arc::new(Mutex::new(vec![]));
		let (etx, erx) = oneshot::channel();
		let _collector = self.record_error(ErrorCollector{collected:collected.clone(), delim: self.error_delimiter.clone(), got_delim: Some(etx)});
		let res = self.execute_expression_raw(expr).await?;
		let _ = self.execute_expression_raw(ERROR_DELIMITER).await?;
		let _ = erx.await;
		if res.is_empty() {
			let c = collected.lock().unwrap();
			if c.is_empty() {
				bail!("expected expression, got nothing")
			}
			bail!("{}", c.join("\n"));
		}
		drop(_collector);
		let Some(res) = res.strip_prefix(&wrapping.0) else {
			bail!("invalid type")
		};
		let Some(res) = res.strip_suffix(&wrapping.1) else {
			bail!("invalid type")
		};
		Ok(res.to_owned())
	}
	async fn execute_expression_empty(&mut self, expr: impl AsRef<[u8]>) -> Result<()> {
		let collected = Arc::new(Mutex::new(vec![]));
		let (etx, erx) = oneshot::channel();
		let _collector = self.record_error(ErrorCollector{collected:collected.clone(), delim: self.error_delimiter.clone(), got_delim: Some(etx)});
		let v = self.execute_expression_raw(expr).await?;
		let _ = self.execute_expression_raw(ERROR_DELIMITER).await;
		let _ = erx.await;

		let c = collected.lock().unwrap();
		if !c.is_empty() {
			bail!("{}", c.join("\n"));
		}
		ensure!(v.is_empty(), "unexpected expression result");
		Ok(())
	}
	async fn execute_expression_raw(&mut self, expr: impl AsRef<[u8]>) -> Result<String> {
		self.send_command(expr).await?;
		// It will be echoed
		self.send_command(REPL_DELIMITER).await?;
		self.read_until_delimiter().await
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

#[derive(Clone, Debug)]
enum Index {
	String(String),
	// Idx(u32),
}
pub struct Field {
	full_path: Vec<Index>,
	session: NixSession,
	value: Option<u32>,
}
impl Field {
	fn root(session: NixSession) -> Self {
		Self {
			full_path: vec![],
			session,
			value: None,
		}
	}
	pub async fn field(session: NixSession, field: &str) -> Result<Self> {
		Self::root(session).get_field_deep([field]).await
	}
	pub async fn get_field(&self, name: &str) -> Result<Self> {
		self.get_field_deep([name]).await
	}
	pub async fn get_field_deep<'a>(
		&self,
		name: impl IntoIterator<Item = &'a str>,
	) -> Result<Self> {
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
			.0
			.lock()
			.await
			.execute_assign(&query)
			.await
			.with_context(|| format!("full path: {:?}", full_path))?;
		Ok(Self {
			full_path,
			session: self.session.clone(),
			value: Some(vid),
		})
	}
	pub async fn as_json<V: DeserializeOwned>(&self) -> Result<V> {
		let id = self.value.expect("can't serialize root field");
		self.session
			.0
			.lock()
			.await
			.execute_expression_to_json(&format!("sess_field_{id}"))
			.await
			.with_context(|| format!("full path: {:?}", self.full_path))
	}
	pub async fn list_fields(&self) -> Result<Vec<String>> {
		let id = self.value.expect("can't list root fields");
		self.session
			.0
			.lock()
			.await
			.execute_expression_to_json(&format!("builtins.attrNames sess_field_{id}"))
			.await
			.with_context(|| format!("full path: {:?}", self.full_path))
	}
}
impl Drop for Field {
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
