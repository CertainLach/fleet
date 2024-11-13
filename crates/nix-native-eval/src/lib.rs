use nixrs::{State, Store};

fn init() {
	nixrs::init();
	let store = Store::new("daemon")?;
	let state = State::new(store)
}
