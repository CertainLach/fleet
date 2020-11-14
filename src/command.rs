use std::{
	ffi::OsStr,
	process::{Command, Stdio},
};

use anyhow::{Context, Result};
use serde::Deserialize;

pub struct CommandOutput(pub Vec<u8>);
impl CommandOutput {
	pub fn into_json<'d, T: Deserialize<'d>>(&'d self) -> Result<T> {
		let str = self.as_str().ok();
		Ok(serde_json::from_slice(&self.0).with_context(|| format!("{:?}", str))?)
	}
	pub fn as_str(&self) -> Result<&str> {
		Ok(std::str::from_utf8(&self.0)?)
	}
}

pub fn ssh_command<I, S>(host: impl AsRef<OsStr>, command: I) -> Result<CommandOutput>
where
	I: IntoIterator<Item = S>,
	S: AsRef<OsStr>,
{
	let out = Command::new("ssh")
		.stderr(Stdio::inherit())
		.arg(host)
		.args(command)
		.output()?;
	if !out.status.success() {
		anyhow::bail!("command failed");
	}
	Ok(CommandOutput(out.stdout))
}
