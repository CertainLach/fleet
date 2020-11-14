use crate::{
	db::{keys::list_hosts, secret::SecretDb, Db, DbData},
	nix::{NixBuild, NixCopy, HOSTS_ATTRIBUTE, SYSTEMS_ATTRIBUTE},
};
use anyhow::Result;
use clap::Clap;
use log::info;

#[derive(Clap)]
pub struct BuildSystems {}

impl BuildSystems {
	pub fn run(self) -> Result<()> {
		let db = Db::new(".fleet")?;
		let hosts = list_hosts()?;
		let data = SecretDb::open(&db)?.generate_nix_data()?;

		for host in hosts.iter() {
			info!("Building host {}", host);
			let path = NixBuild::new(format!(
				"{}.{}.config.system.build.toplevel",
				SYSTEMS_ATTRIBUTE, host,
			))
			.env("SECRET_DATA".into(), data.clone())
			.run()?;
			info!("{:?}", path.path());
			NixCopy::new(path.path().to_owned()).to(format!("ssh://root@{}", host))?;
			std::thread::sleep_ms(9999999)
		}
		Ok(())
	}
}
