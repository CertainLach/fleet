//! This whole library should be replaced with either binding to nix libexpr,
//! or with tvix (once it is able to build NixOS).
//!
//! Current api is awful, little effort was put into this implementation.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub use pool::NixSessionPool;
use pool::NixSessionPoolInner;
use r2d2::PooledConnection;
pub use session::{Error, Result};
use tokio::{
	sync::{mpsc, oneshot},
	task::AbortHandle,
};
use tracing::{info, instrument, Instrument};
pub use value::{Index, Value};

mod pool;
mod session;
mod value;
// Contains macros helpers
#[doc(hidden)]
pub mod macros;
pub mod util;
// #[allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]
// mod nix_raw {
// 	include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
// }

// fn init() {
// 	nix_raw::libutil_init();
// }

#[derive(Clone)]
pub struct NixSession(pub(crate) Arc<tokio::sync::Mutex<PooledConnection<NixSessionPoolInner>>>);

struct NixBuildTask(Value, oneshot::Sender<Result<HashMap<String, PathBuf>>>);

#[derive(Clone)]
pub struct NixBuildBatch {
	tx: mpsc::UnboundedSender<NixBuildTask>,
}

#[instrument(skip(session, values))]
async fn build_multiple(name: String, session: NixSession, values: Vec<Value>) -> Result<()> {
	let builtins = Value::binding(session, "builtins").await?;
	let system = nix_go!(builtins.currentSystem);
	let drv = nix_go!(builtins.derivation(Obj {
		system,
		name,
		builder: "/bin/sh",
		// we want nothing from this derivation, it is only used to perform multiple builds at once.
		args: vec!["-c", "echo > $out"],
		preferLocalBuild: true,
		allowSubstitutes: false,
		buildInputs: values,
	}));
	drv.build().await?;
	Ok(())
}

impl NixBuildBatch {
	fn new(name: String, session: NixSession) -> Self {
		let (tx, mut rx) = mpsc::unbounded_channel::<NixBuildTask>();

		tokio::task::spawn(async move {
			let mut deps = vec![];
			let mut build_data = vec![];
			while let Some(task) = rx.recv().await {
				build_data.push(task.0.clone());
				deps.push(task);
			}
			if deps.is_empty() {
				return;
			}
			match build_multiple(name, session, build_data).await {
				Ok(_) => {
					for NixBuildTask(v, o) in deps {
						let _ = o.send(v.build().await);
					}
				}
				Err(e) => {
					for NixBuildTask(v, o) in deps {
						let s = v.to_string_weak().await.expect("drv is string-like");
						if PathBuf::from(s).exists() {
							let _ = o.send(v.build().await);
						} else {
							let _ = o.send(Err(e.clone()));
						}
					}
				}
			};
		});
		Self { tx }
	}
	pub async fn submit(self, task: Value) -> Result<HashMap<String, PathBuf>> {
		let Self { tx: task_tx } = self;
		let (tx, rx) = oneshot::channel();
		let _ = task_tx.send(NixBuildTask(task, tx));
		drop(task_tx);
		rx.await.expect("shoudn't be cancelled here")
	}
}

impl NixSession {
	fn ptr_eq(a: &Self, b: &Self) -> bool {
		Arc::ptr_eq(&a.0, &b.0)
	}

	pub fn new_build_batch(&self, name: String) -> NixBuildBatch {
		NixBuildBatch::new(name, self.clone())
	}
}

pub fn init_tokio() {
	let _ = pool::TOKIO_RUNTIME.set(tokio::runtime::Handle::current());
}
