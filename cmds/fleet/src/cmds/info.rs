use std::{collections::BTreeSet, time::Duration};

use crate::{command::CommandExt, host::Config};
use anyhow::{bail, ensure, Result};
use clap::Parser;
use nixlike::format_nix;
use serde_json::{json, Value};
use tokio::{
	fs::{self, File},
	io::AsyncWriteExt,
	process::Command,
};

#[derive(Parser)]
pub struct Info {
	#[clap(long)]
	json: bool,
	#[clap(subcommand)]
	cmd: InfoCmd,
}

#[derive(Parser)]
pub enum InfoCmd {
	/// List hosts
	ListHosts {
		#[clap(long)]
		tagged: Vec<String>,
	},
	/// List ips
	HostIps {
		host: String,
		#[clap(long)]
		external: bool,
		#[clap(long)]
		internal: bool,
	},
}

impl Info {
	pub async fn run(self, config: &Config) -> Result<()> {
		let mut data = Vec::new();
		match self.cmd {
			InfoCmd::ListHosts { ref tagged } => {
				'host: for host in config.list_hosts().await? {
					if !tagged.is_empty() {
						let tags: Vec<String> = config.config_attr(&host, "tags").await?;
						for tag in tagged {
							if !tags.contains(tag) {
								continue 'host;
							}
						}
					}
					data.push(host);
				}
			}
			InfoCmd::HostIps {
				host,
				external,
				internal,
			} => {
				ensure!(
					external || internal,
					"at leas one of --external or --internal must be set"
				);
				let mut out = <BTreeSet<String>>::new();
				if external {
					out.extend(
						config
							.config_attr::<Vec<String>>(&host, "network.externalIps")
							.await?,
					);
				}
				if internal {
					out.extend(
						config
							.config_attr::<Vec<String>>(&host, "network.internalIps")
							.await?,
					);
				}
				for ip in out {
					data.push(ip);
				}
			}
		}

		if self.json {
			let v = serde_json::to_string_pretty(&data)?;
			print!("{}", v);
		} else {
			for v in data {
				println!("{}", v);
			}
		}
		Ok(())
	}
}
