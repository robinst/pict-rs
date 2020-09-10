use crate::UploadError;
use sled as sled034;
use sled032;
use std::path::PathBuf;
use tracing::{debug, info, warn};

const SLED_034: &str = "db-0.34";
const SLED_032: &str = "db-0.32";
const SLED_0320_RC1: &str = "db";

pub(crate) struct LatestDb {
    root_dir: PathBuf,
    version: DbVersion,
}

impl LatestDb {
    pub(crate) fn exists(root_dir: PathBuf) -> Self {
        let version = DbVersion::exists(root_dir.clone());

        LatestDb { root_dir, version }
    }

    pub(crate) fn migrate(self) -> Result<sled034::Db, UploadError> {
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
        let mut sled_dir = root.clone();
        sled_dir.push("sled");
        sled_dir.push(SLED_034);
        if std::fs::metadata(sled_dir).is_ok() {
            return DbVersion::Sled034;
        }

        let mut sled_dir = root.clone();
        sled_dir.push("sled");
        sled_dir.push(SLED_032);
        if std::fs::metadata(sled_dir).is_ok() {
            return DbVersion::Sled032;
        }

        let mut sled_dir = root;
        sled_dir.push(SLED_0320_RC1);
        if std::fs::metadata(sled_dir).is_ok() {
            return DbVersion::Sled0320Rc1;
        }

        DbVersion::Fresh
    }

    fn migrate(self, root: PathBuf) -> Result<sled034::Db, UploadError> {
        match self {
            DbVersion::Sled0320Rc1 => {
                migrate_0_32_0_rc1(root.clone())?;
                migrate_0_32(root)
            }
            DbVersion::Sled032 => migrate_0_32(root),
            DbVersion::Sled034 | DbVersion::Fresh => {
                let mut sled_dir = root;
                sled_dir.push("sled");
                sled_dir.push(SLED_034);
                Ok(sled034::open(sled_dir)?)
            }
        }
    }
}

fn migrate_0_32(mut root: PathBuf) -> Result<sled034::Db, UploadError> {
    info!("Migrating database from 0.32 to 0.34");
    root.push("sled");

    let mut sled_dir = root.clone();
    sled_dir.push(SLED_032);

    let mut new_sled_dir = root.clone();
    new_sled_dir.push(SLED_034);

    let old_db = sled032::open(sled_dir)?;
    let new_db = sled034::open(new_sled_dir)?;

    new_db.import(old_db.export());

    Ok(new_db)
}

fn migrate_0_32_0_rc1(root: PathBuf) -> Result<sled032::Db, UploadError> {
    info!("Migrating database from 0.32.0-rc1 to 0.32.0");
    let mut sled_dir = root.clone();
    sled_dir.push("db");

    let mut new_sled_dir = root;
    new_sled_dir.push("sled");
    new_sled_dir.push(SLED_032);

    let old_db = sled032::open(sled_dir)?;
    let new_db = sled032::open(new_sled_dir)?;

    let old_alias_tree = old_db.open_tree("alias")?;
    let new_alias_tree = new_db.open_tree("alias")?;

    let old_fname_tree = old_db.open_tree("filename")?;
    let new_fname_tree = new_db.open_tree("filename")?;

    for res in old_alias_tree.iter().keys() {
        let k = res?;
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

    for res in old_fname_tree.iter().keys() {
        let k = res?;
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

    Ok(new_db) as Result<sled032::Db, UploadError>
}

fn migrate_main_tree(
    alias: &sled032::IVec,
    hash: &sled032::IVec,
    old_db: &sled032::Db,
    new_db: &sled032::Db,
) -> Result<(), UploadError> {
    debug!(
        "Migrating files for {}",
        String::from_utf8_lossy(alias.as_ref())
    );
    if let Some(v) = old_db.get(&hash)? {
        new_db.insert(&hash, v)?;
    } else {
        warn!("Missing filename");
    }

    let (start, end) = alias_key_bounds(&hash);
    for res in old_db.range(start..end) {
        let (k, v) = res?;
        debug!("Moving alias {}", String::from_utf8_lossy(v.as_ref()));
        new_db.insert(k, v)?;
    }

    let (start, end) = variant_key_bounds(&hash);
    for res in old_db.range(start..end) {
        let (k, v) = res?;
        debug!("Moving variant {}", String::from_utf8_lossy(v.as_ref()));
        new_db.insert(k, v)?;
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
