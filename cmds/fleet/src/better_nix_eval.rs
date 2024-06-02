//! Wrapper around nix repl, which allows to work on nix code, without relying on
//! nix libexpr. I mean, nix libexpr is good, but until it has no C bindings, this is the royal PITA.

use std::{
	collections::HashMap,
	ffi::{OsStr, OsString},
	fmt::{self, Display},
	path::PathBuf,
	process::Stdio,
	sync::{Arc, OnceLock},
};

use anyhow::{anyhow, bail, ensure, Context, Result};
use better_command::{ClonableHandler, Handler, NixHandler, NoopHandler};
use futures::StreamExt;
use itertools::Itertools;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{
	io::AsyncWriteExt,
	process::{ChildStderr, ChildStdin, ChildStdout, Command},
	select,
	sync::{mpsc, oneshot, Mutex},
};
use tracing::{debug, error, warn, Level};
