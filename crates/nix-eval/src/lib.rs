//! This whole library should be replaced with either binding to nix libexpr,
//! or with tvix (once it is able to build NixOS).
//!
//! Current api is awful, little effort was put into this implementation.

use std::sync::Arc;

pub use pool::NixSessionPool;
use pool::NixSessionPoolInner;
use r2d2::PooledConnection;
pub use session::{Error, Result};
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

impl NixSession {
	fn ptr_eq(a: &Self, b: &Self) -> bool {
		Arc::ptr_eq(&a.0, &b.0)
	}
}

pub fn init_tokio() {
	let _ = pool::TOKIO_RUNTIME.set(tokio::runtime::Handle::current());
}
