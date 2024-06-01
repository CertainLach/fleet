use std::io::stdout;

use clap::{Command, Parser};
use clap_complete::Shell;

#[derive(Parser)]
pub struct Complete {
	/// For which shell to generate the completions
	#[arg(long, short)]
	shell: Shell,
}

impl Complete {
	pub fn run(&self, mut command: Command) {
		let Self { shell } = self;

		let bin_name = command
			.get_bin_name()
			.unwrap_or_else(|| command.get_name())
			.to_owned();
		clap_complete::generate(*shell, &mut command, &bin_name, &mut stdout());
	}
}
