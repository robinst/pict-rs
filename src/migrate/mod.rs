use crate::UploadError;
use sled;
use std::path::PathBuf;
use tracing::{debug, info, warn};

mod s032;
mod s034;

type SledIter = Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), UploadError>>>;

trait SledDb {
    type SledTree: SledTree;

    fn open_tree(&self, name: &str) -> Result<Self::SledTree, UploadError>;

    fn self_tree(&self) -> &Self::SledTree;
}

impl<T> SledDb for &T
where
    T: SledDb,
{
    type SledTree = T::SledTree;

    fn open_tree(&self, name: &str) -> Result<Self::SledTree, UploadError> {
        (*self).open_tree(name)
    }

    fn self_tree(&self) -> &Self::SledTree {
        (*self).self_tree()
    }
}

trait SledTree {
    fn get<K>(&self, key: K) -> Result<Option<Vec<u8>>, UploadError>
    where
        K: AsRef<[u8]>;

    fn insert<K, V>(&self, key: K, value: V) -> Result<(), UploadError>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>;

    fn iter(&self) -> SledIter;

    fn range<K, R>(&self, range: R) -> SledIter
    where
        K: AsRef<[u8]>,
        R: std::ops::RangeBounds<K>;
}

pub(crate) struct LatestDb {
    root_dir: PathBuf,
    version: DbVersion,
}

impl LatestDb {
    pub(crate) fn exists(root_dir: PathBuf) -> Self {
        let version = DbVersion::exists(root_dir.clone());

        LatestDb { root_dir, version }
    }

    pub(crate) fn migrate(self) -> Result<sled::Db, UploadError> {
        let LatestDb { root_dir, version } = self;

        version.migrate(root_dir)
    }
}

enum DbVersion {
    Sled0320Rc1,
    Sled032,
    Sled034,
    Fresh,
}

impl DbVersion {
    fn exists(root: PathBuf) -> Self {
        if s034::exists(root.clone()) {
            return DbVersion::Sled034;
        }

        if s032::exists(root.clone()) {
            return DbVersion::Sled032;
        }

        if s032::exists_rc1(root.clone()) {
            return DbVersion::Sled0320Rc1;
        }

        DbVersion::Fresh
    }

    fn migrate(self, root: PathBuf) -> Result<sled::Db, UploadError> {
        match self {
            DbVersion::Sled0320Rc1 => migrate_0_32_0_rc1(root),
            DbVersion::Sled032 => migrate_0_32(root),
            DbVersion::Sled034 | DbVersion::Fresh => s034::open(root),
        }
    }
}

fn migrate_0_32_0_rc1(root: PathBuf) -> Result<sled::Db, UploadError> {
    info!("Migrating database from 0.32.0-rc1 to 0.34.0");

    let old_db = s032::open_rc1(root.clone())?;
    let new_db = s034::open(root)?;

    migrate(old_db, new_db)
}

fn migrate_0_32(root: PathBuf) -> Result<sled::Db, UploadError> {
    info!("Migrating database from 0.32 to 0.34");

    let old_db = s032::open(root.clone())?;
    let new_db = s034::open(root)?;

    migrate(old_db, new_db)
}

fn migrate<Old, New>(old_db: Old, new_db: New) -> Result<New, UploadError>
where
    Old: SledDb,
    New: SledDb,
{
    let old_alias_tree = old_db.open_tree("alias")?;
    let new_alias_tree = new_db.open_tree("alias")?;

    let old_fname_tree = old_db.open_tree("filename")?;
    let new_fname_tree = new_db.open_tree("filename")?;

    for res in old_alias_tree.iter() {
        let (k, _) = res?;

        if let Some(v) = old_alias_tree.get(&k)? {
            if !k.contains(&b"/"[0]) {
                // k is an alias
                migrate_main_tree(&k, &v, &old_db, &new_db)?;
                debug!(
                    "Moving alias -> hash for alias {}",
                    String::from_utf8_lossy(k.as_ref()),
                );
            } else {
                debug!(
                    "Moving {}, {}",
                    String::from_utf8_lossy(k.as_ref()),
                    String::from_utf8_lossy(v.as_ref())
                );
            }
            new_alias_tree.insert(k, v)?;
        } else {
            warn!("MISSING {}", String::from_utf8_lossy(k.as_ref()));
        }
    }

    for res in old_fname_tree.iter() {
        let (k, _) = res?;
        if let Some(v) = old_fname_tree.get(&k)? {
            debug!(
                "Moving file -> hash for file {}",
                String::from_utf8_lossy(k.as_ref()),
            );
            new_fname_tree.insert(&k, &v)?;
        } else {
            warn!("MISSING {}", String::from_utf8_lossy(k.as_ref()));
        }
    }

    Ok(new_db)
}

fn migrate_main_tree<Old, New>(
    alias: &[u8],
    hash: &[u8],
    old_db: Old,
    new_db: New,
) -> Result<(), UploadError>
where
    Old: SledDb,
    New: SledDb,
{
    let main_tree = new_db.open_tree("main")?;

    debug!(
        "Migrating files for {}",
        String::from_utf8_lossy(alias.as_ref())
    );
    if let Some(v) = old_db.self_tree().get(&hash)? {
        main_tree.insert(&hash, v)?;
    } else {
        warn!("Missing filename");
    }

    let (start, end) = alias_key_bounds(&hash);
    for res in old_db.self_tree().range(start..end) {
        let (k, v) = res?;
        debug!("Moving alias {}", String::from_utf8_lossy(v.as_ref()));
        main_tree.insert(k, v)?;
    }

    let (start, end) = variant_key_bounds(&hash);
    for res in old_db.self_tree().range(start..end) {
        let (k, v) = res?;
        debug!("Moving variant {}", String::from_utf8_lossy(v.as_ref()));
        main_tree.insert(k, v)?;
    }

    Ok(())
}

pub(crate) fn alias_key_bounds(hash: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut start = hash.to_vec();
    start.extend(&[0]);

    let mut end = hash.to_vec();
    end.extend(&[1]);

    (start, end)
}

pub(crate) fn variant_key_bounds(hash: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut start = hash.to_vec();
    start.extend(&[2]);

    let mut end = hash.to_vec();
    end.extend(&[3]);

    (start, end)
}
