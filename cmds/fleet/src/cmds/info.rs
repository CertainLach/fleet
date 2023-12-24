use std::collections::BTreeSet;

use crate::host::Config;
use crate::nix_path;
use anyhow::{ensure, Result};
use clap::Parser;

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
						let tags: Vec<String> = config
							.fleet_field
							.select(nix_path!(.configuredSystems.{&host.name}.config.tags))
							.await?
							.as_json()
							.await?;
						for tag in tagged {
							if !tags.contains(tag) {
								continue 'host;
							}
						}
					}
					data.push(host.name);
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
				let host = config.system_config(&host).await?;
				if external {
					out.extend(
						host.select(nix_path!(.network.externalIps))
							.await?
							.as_json::<Vec<String>>()
							.await?,
					);
				}
				if internal {
					out.extend(
						host.select(nix_path!(.network.internalIps))
							.await?
							.as_json::<Vec<String>>()
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
