use std::{
	collections::BTreeMap,
	env::current_dir,
	ffi::OsString,
	str::FromStr,
	sync::{Arc, Mutex},
};

use anyhow::Result;
use clap::Parser;
use nix_eval::{nix_go, util::assert_warn, NixSessionPool, Value};
use nom::{
	bytes::complete::take_while1,
	character::complete::char,
	combinator::{map, opt},
	multi::separated_list1,
	sequence::{preceded, separated_pair},
};

use crate::{
	fleetdata::FleetData,
	host::{Config, ConfigHost, FleetConfigInternals},
};

#[derive(Clone)]
pub enum HostItem {
	Host {
		name: String,
		attrs: BTreeMap<String, String>,
	},
	Tag {
		name: String,
		attrs: BTreeMap<String, String>,
	},
}
fn host_item_parser(input: &str) -> Result<HostItem, String> {
	fn err_to_string(err: nom::Err<nom::error::Error<&str>>) -> String {
		err.to_string()
	}

	let (input, is_tag) = map(opt(char('@')), |c| c.is_some())(input).map_err(err_to_string)?;
	let (input, name) = map(
		take_while1(|v| v != ',' && v != '?' && v != '@'),
		str::to_owned,
	)(input)
	.map_err(err_to_string)?;

	let kw_item = separated_pair(
		map(take_while1(|v| v != '&' && v != '='), str::to_owned),
		char('='),
		map(take_while1(|v| v != '&'), str::to_owned),
	);
	let kw = map(separated_list1(char('&'), kw_item), |vec| {
		vec.into_iter().collect::<BTreeMap<_, _>>()
	});
	let mut opt_kw = map(opt(preceded(char('?'), kw)), Option::unwrap_or_default);

	let (input, attrs) = opt_kw(input).map_err(err_to_string)?;

	if !input.is_empty() {
		return Err(format!("unexpected trailing input: {input:?}"));
	}
	Ok(if is_tag {
		HostItem::Tag { name, attrs }
	} else {
		HostItem::Host { name, attrs }
	})
}

// TODO: Rename to HostSelector
#[derive(Parser, Clone)]
pub struct FleetOpts {
	/// All hosts except those would be skipped
	#[clap(long, number_of_values = 1, value_parser = host_item_parser)]
	pub only: Vec<HostItem>,

	/// Hosts to skip
	#[clap(long, number_of_values = 1)]
	pub skip: Vec<String>,

	/// Host, which should be threaten as current machine
	// TODO: Replace with connectivity refactor
	#[clap(long, default_value_t = hostname::get().expect("unknown hostname").to_str().expect("hostname is not utf-8").to_owned())]
	pub localhost: String,

	/// Override detected system for host, to perform builds via
	/// binfmt-declared qemu instead of trying to crosscompile
	#[clap(long, default_value = env!("NIX_SYSTEM"))]
	pub local_system: String,
}

impl FleetOpts {
	pub async fn filter_skipped(
		&self,
		hosts: impl IntoIterator<Item = ConfigHost>,
	) -> Result<Vec<ConfigHost>> {
		let mut out = Vec::new();
		for host in hosts {
			if self.should_skip(&host).await? {
				continue;
			}
			out.push(host);
		}
		Ok(out)
	}
	pub async fn should_skip(&self, host: &ConfigHost) -> Result<bool> {
		if self.skip.iter().any(|h| h as &str == host.name) {
			return Ok(true);
		}
		if self.only.is_empty() {
			return Ok(false);
		}
		let mut have_group_matches = false;
		for item in self.only.iter() {
			match item {
				HostItem::Host { name, .. } if *name == host.name => {
					return Ok(false);
				}
				HostItem::Tag { .. } => {
					have_group_matches = true;
				}
				_ => {}
			}
		}
		if have_group_matches {
			let host_tags = host.tags().await?;
			for item in self.only.iter() {
				match item {
					HostItem::Tag { name, .. } if host_tags.contains(name) => {
						return Ok(false);
					}
					_ => {}
				}
			}
		}
		Ok(true)
	}
	pub async fn action_attr<T: FromStr>(&self, host: &ConfigHost, attr: &str) -> Result<Option<T>>
	where
		T::Err: Sync,
		anyhow::Error: From<T::Err>,
	{
		let str = self.action_attr_str(host, attr).await?;
		Ok(str.map(|v| T::from_str(&v)).transpose()?)
	}
	pub async fn action_attr_str(&self, host: &ConfigHost, attr: &str) -> Result<Option<String>> {
		if self.only.is_empty() {
			return Ok(None);
		}
		let mut have_group_matches = false;
		for item in self.only.iter() {
			match item {
				HostItem::Host { name, attrs }
					if *name == host.name && attrs.contains_key(attr) =>
				{
					return Ok(attrs.get(attr).cloned());
				}
				HostItem::Tag { attrs, .. } if attrs.contains_key(attr) => {
					have_group_matches = true;
				}
				_ => {}
			}
		}
		if have_group_matches {
			let host_tags = host.tags().await?;
			for item in self.only.iter() {
				match item {
					HostItem::Tag { name, attrs }
						if host_tags.contains(name) && attrs.contains_key(attr) =>
					{
						return Ok(attrs.get(attr).cloned());
					}
					_ => {}
				}
			}
		}
		Ok(None)
	}
	pub fn is_local(&self, host: &str) -> bool {
		self.localhost == host
	}

	// TODO: Config should be detached from opts.
	pub async fn build(&self, nix_args: Vec<OsString>, assert: bool) -> Result<Config> {
		let directory = current_dir()?;

		let pool = NixSessionPool::new(
			directory.as_os_str().to_owned(),
			nix_args.clone(),
			self.local_system.clone(),
		)
		.await?;
		let nix_session = pool.get().await?;

		let builtins_field = Value::binding(nix_session.clone(), "builtins").await?;

		let mut fleet_data_path = directory.clone();
		fleet_data_path.push("fleet.nix");
		let bytes = std::fs::read_to_string(fleet_data_path)?;
		let data: Mutex<FleetData> = nixlike::parse_str(&bytes)?;

		let fleet_root = Value::binding(nix_session.clone(), "fleetConfigurations").await?;
		let fleet_field = nix_go!(fleet_root.default({ data }));

		let config_field = nix_go!(fleet_field.config);

		if assert {
			assert_warn("fleet config evaluation", &config_field).await?;
		}

		let import = nix_go!(builtins_field.import);
		let overlays = nix_go!(config_field.nixpkgs.overlays);
		let nixpkgs = nix_go!(config_field.nixpkgs.buildUsing | import);

		let default_pkgs = nix_go!(nixpkgs(Obj {
			overlays,
			system: self.local_system.clone(),
		}));

		Ok(Config(Arc::new(FleetConfigInternals {
			nix_session,
			directory,
			data,
			local_system: self.local_system.clone(),
			nix_args,
			config_field,
			default_pkgs,
			localhost: self.localhost.to_owned(),
		})))
	}
}
