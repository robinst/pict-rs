use crate::{
    error::Error,
    migrate::{SledDb, SledIter, SledTree},
};
use sled as sled034;
use std::path::PathBuf;

const SLED_034: &str = "db-0.34";

pub(crate) fn exists(mut base: PathBuf) -> bool {
    base.push("sled");
    base.push(SLED_034);

    std::fs::metadata(base).is_ok()
}

pub(crate) fn migrating(base: PathBuf) -> bool {
    if let Ok(db) = open(base) {
        if let Ok(tree) = db.open_tree("migrate") {
            if let Ok(Some(_)) = tree.get("done") {
                return false;
            }
        }
    }

    true
}

pub(crate) fn open(mut base: PathBuf) -> Result<sled034::Db, Error> {
    base.push("sled");
    base.push(SLED_034);

    let db = sled034::Config::default()
        .cache_capacity(1024 * 1024 * 64)
        .path(base)
        .open()?;

    Ok(db)
}

impl SledDb for sled034::Db {
    type SledTree = sled034::Tree;

    fn open_tree(&self, name: &str) -> Result<Self::SledTree, Error> {
        Ok(sled034::Db::open_tree(self, name)?)
    }

    fn self_tree(&self) -> &Self::SledTree {
        &*self
    }
}

impl SledTree for sled034::Tree {
    fn get<K>(&self, key: K) -> Result<Option<Vec<u8>>, Error>
    where
        K: AsRef<[u8]>,
    {
        Ok(sled034::Tree::get(self, key)?.map(|v| Vec::from(v.as_ref())))
    }

    fn insert<K, V>(&self, key: K, value: V) -> Result<(), Error>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        Ok(sled034::Tree::insert(self, key, value.as_ref().to_vec()).map(|_| ())?)
    }

    fn iter(&self) -> SledIter {
        Box::new(sled034::Tree::iter(self).map(|res| {
            res.map(|(k, v)| (k.as_ref().to_vec(), v.as_ref().to_vec()))
                .map_err(Error::from)
        }))
    }

    fn range<K, R>(&self, range: R) -> SledIter
    where
        K: AsRef<[u8]>,
        R: std::ops::RangeBounds<K>,
    {
        Box::new(sled034::Tree::range(self, range).map(|res| {
            res.map(|(k, v)| (k.as_ref().to_vec(), v.as_ref().to_vec()))
                .map_err(Error::from)
        }))
    }

    fn flush(&self) -> Result<(), Error> {
        sled034::Tree::flush(self).map(|_| ()).map_err(Error::from)
    }
}
