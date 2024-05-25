//! Wrapper around nix repl, which allows to work on nix code, without relying on
//! nix libexpr. I mean, nix libexpr is good, but until it has no C bindings, this is the royal PITA.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, bail, ensure, Context, Result};
use better_command::{ClonableHandler, Handler, NixHandler, NoopHandler};
use futures::StreamExt;
use itertools::Itertools;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::select;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, warn, Level};




