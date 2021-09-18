//! Small .toml based readable data store

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::{
	cell::Cell,
	collections::HashSet,
	io::Write,
	ops::{Deref, DerefMut},
	path::Path,
	path::PathBuf,
	sync::{Arc, Mutex},
};

struct DbInternal {
	root: PathBuf,
	locked_paths: HashSet<PathBuf>,
}

pub trait DbData: DeserializeOwned + Serialize + Default {
	const DB_NAME: &'static str;

	fn open(db: &Db) -> Result<DbFile<Self>> {
		db.db::<Self>()
	}
}

#[derive(Clone)]
pub struct Db(Arc<Mutex<DbInternal>>);
impl Db {
	pub fn new(root: impl AsRef<Path>) -> Result<Self> {
		let root: &Path = root.as_ref();
		std::fs::create_dir_all(&root).context("db root")?;
		Ok(Db(Arc::new(Mutex::new(DbInternal {
			root: root.to_owned(),
			locked_paths: HashSet::new(),
		}))))
	}

	pub fn db<T: DbData>(&self) -> Result<DbFile<T>> {
		let name = T::DB_NAME;
		assert!(!name.contains('/') && !name.contains('\\'));
		let mut db = self.0.lock().unwrap();
		let mut data_path = db.root.clone();
		data_path.push(format!("{}.toml", name));

		if !db.locked_paths.insert(data_path.clone()) {
			anyhow::bail!("file is already open");
		}

		let data = if data_path.exists() {
			let raw_data = std::fs::read(&data_path).context("reading file")?;
			toml::from_slice(&raw_data).context("parsing file")?
		} else {
			T::default()
		};

		Ok(DbFile {
			db: self.clone(),
			root: db.root.clone(),
			path: data_path,
			data,
			dirty: Cell::new(false),
		})
	}
}

pub struct DbFile<T: DbData> {
	db: Db,
	root: PathBuf,
	path: PathBuf,
	data: T,
	dirty: Cell<bool>,
}

impl<T: DbData> Deref for DbFile<T> {
	type Target = T;

	fn deref(&self) -> &Self::Target {
		&self.data
	}
}

impl<T: DbData> DerefMut for DbFile<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		self.dirty.set(true);
		&mut self.data
	}
}

impl<T: DbData> DbFile<T> {
	pub fn write(&self) -> Result<()> {
		if !self.dirty.get() {
			return Ok(());
		}
		let mut temp = tempfile::Builder::new()
			.prefix("~")
			.suffix(".toml")
			.tempfile_in(&self.root)?;
		let mut out = String::new();
		let mut serializer = toml::Serializer::new(&mut out);
		serializer.pretty_array(true).pretty_string(true);
		self.data.serialize(&mut serializer)?;
		temp.write_all(out.as_bytes())?;
		temp.persist(&self.path)?;
		self.dirty.set(false);
		Ok(())
	}
}

impl<T: DbData> Drop for DbFile<T> {
	fn drop(&mut self) {
		let mut db = self.db.0.lock().unwrap();
		self.write().unwrap();
		db.locked_paths.remove(&self.path);
	}
}
