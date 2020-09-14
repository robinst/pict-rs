use crate::{
    migrate::{SledDb, SledIter, SledTree},
    UploadError,
};
use std::path::PathBuf;

const SLED_032: &str = "db-0.32";
const SLED_0320_RC1: &str = "db";

pub(crate) fn exists_rc1(mut base: PathBuf) -> bool {
    base.push(SLED_0320_RC1);

    std::fs::metadata(base).is_ok()
}

pub(crate) fn open_rc1(mut base: PathBuf) -> Result<sled032::Db, UploadError> {
    base.push(SLED_0320_RC1);

    Ok(sled032::open(base)?)
}

pub(crate) fn exists(mut base: PathBuf) -> bool {
    base.push("sled");
    base.push(SLED_032);

    std::fs::metadata(base).is_ok()
}

pub(crate) fn open(mut base: PathBuf) -> Result<sled032::Db, UploadError> {
    base.push("sled");
    base.push(SLED_032);

    Ok(sled032::open(base)?)
}

impl SledDb for sled032::Db {
    type SledTree = sled032::Tree;

    fn open_tree(&self, name: &str) -> Result<Self::SledTree, UploadError> {
        Ok(sled032::Db::open_tree(self, name)?)
    }

    fn self_tree(&self) -> &Self::SledTree {
        &*self
    }
}

impl SledTree for sled032::Tree {
    fn get<K>(&self, key: K) -> Result<Option<Vec<u8>>, UploadError>
    where
        K: AsRef<[u8]>,
    {
        Ok(sled032::Tree::get(self, key)?.map(|v| Vec::from(v.as_ref())))
    }

    fn insert<K, V>(&self, key: K, value: V) -> Result<(), UploadError>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        Ok(sled032::Tree::insert(self, key, value.as_ref().to_vec()).map(|_| ())?)
    }

    fn iter(&self) -> SledIter {
        Box::new(sled032::Tree::iter(self).map(|res| {
            res.map(|(k, v)| (k.as_ref().to_vec(), v.as_ref().to_vec()))
                .map_err(UploadError::from)
        }))
    }

    fn range<K, R>(&self, range: R) -> SledIter
    where
        K: AsRef<[u8]>,
        R: std::ops::RangeBounds<K>,
    {
        Box::new(sled032::Tree::range(self, range).map(|res| {
            res.map(|(k, v)| (k.as_ref().to_vec(), v.as_ref().to_vec()))
                .map_err(UploadError::from)
        }))
    }
}
