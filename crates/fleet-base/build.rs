use std::env;

fn main() {
	let target = env::var("TARGET").expect("TARGET env var is set by cargo");

	let nix_system = if target.starts_with("x86_64-unknown-linux-") {
		"x86_64-linux"
	} else if target.starts_with("aarch64-unknown-linux-") {
		"aarch64-linux"
	} else {
		panic!("unknown nix system name for rust {target} triple!");
	};

	println!("cargo:rustc-env=NIX_SYSTEM={nix_system}");
}
