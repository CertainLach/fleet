use anyhow::bail;
use tracing::{debug, warn};
use std::time::Instant;

use crate::{nix_go_json, Value};

pub async fn assert_warn(action: &str, val: &Value) -> anyhow::Result<()> {
	let before_errors = Instant::now();
	let errors: Vec<String> = nix_go_json!(val.errors);
	debug!("errors evaluation took {:?}", before_errors.elapsed());
	if !errors.is_empty() {
		bail!(
			"{action} failed with error{}{}",
			(errors.len() != 1).then_some("s:\n- ").unwrap_or(": "),
			errors.join("\n- "),
		);
	}

	let before_errors = Instant::now();
	let warnings: Vec<String> = nix_go_json!(val.warnings);
	debug!("warnings evaluation took {:?}", before_errors.elapsed());
	if !warnings.is_empty() {
		warn!(
			"{action} completed with warning{}{}",
			(warnings.len() != 1).then_some("s:\n- ").unwrap_or(": "),
			warnings.join("\n- "),
		);
	}
	Ok(())
}
