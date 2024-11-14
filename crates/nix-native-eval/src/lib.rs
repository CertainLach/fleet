use anyhow::Result;
use nixrs::{State, Store};

pub fn init() -> Result<()> {
	nixrs::init()?;
	let store = Store::new("daemon")?;
	let state = State::new(store)?;
	let _ = state;

	Ok(())
}
