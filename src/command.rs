use std::{
	ffi::OsStr,
	process::{Command, Stdio},
};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

pub trait CommandExt {
	fn run(&mut self) -> Result<()>;
	fn run_json<T: DeserializeOwned>(&mut self) -> Result<T>;
	fn run_string(&mut self) -> Result<String>;
	fn inherit_stdio(&mut self) -> &mut Self;
	fn ssh_on(host: impl AsRef<OsStr>, command: impl AsRef<OsStr>) -> Self;
}

impl CommandExt for Command {
	fn inherit_stdio(&mut self) -> &mut Self {
		self.stderr(Stdio::inherit());
		self
	}

	fn run(&mut self) -> Result<()> {
		let out = self.output()?;
		if !out.status.success() {
			anyhow::bail!("command failed with status {}", out.status);
		}
		Ok(())
	}

	fn run_json<T: DeserializeOwned>(&mut self) -> Result<T> {
		let str = self.run_string()?;
		serde_json::from_str(&str).with_context(|| format!("{:?}", str))
	}

	fn run_string(&mut self) -> Result<String> {
		let out = self.output()?;
		if !out.status.success() {
			anyhow::bail!("command failed");
		}
		Ok(String::from_utf8(out.stdout)?)
	}

	fn ssh_on(host: impl AsRef<OsStr>, command: impl AsRef<OsStr>) -> Self {
		let mut cmd = Command::new("ssh");
		cmd.arg(host).arg("--").arg(command);
		cmd
	}
}
