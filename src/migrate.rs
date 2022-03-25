use crate::UploadError;
use std::path::PathBuf;

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

    fn flush(&self) -> Result<(), UploadError>;
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

        loop {
            let root_dir2 = root_dir.clone();
            let res = std::panic::catch_unwind(move || version.migrate(root_dir2));

            if let Ok(res) = res {
                return res;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum DbVersion {
    Sled034,
    Fresh,
}

impl DbVersion {
    fn exists(root: PathBuf) -> Self {
        if s034::exists(root.clone()) && !s034::migrating(root) {
            return DbVersion::Sled034;
        }

        DbVersion::Fresh
    }

    fn migrate(self, root: PathBuf) -> Result<sled::Db, UploadError> {
        match self {
            DbVersion::Sled034 | DbVersion::Fresh => s034::open(root),
        }
    }
}

pub(crate) fn alias_key_bounds(hash: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut start = hash.to_vec();
    start.extend(&[0]);

    let mut end = hash.to_vec();
    end.extend(&[1]);

    (start, end)
}

pub(crate) fn alias_id_key(alias: &str) -> String {
    format!("{}/id", alias)
}

pub(crate) fn alias_key(hash: &[u8], id: &str) -> Vec<u8> {
    let mut key = hash.to_vec();
    // add a separator to the key between the hash and the ID
    key.extend(&[0]);
    key.extend(id.as_bytes());

    key
}
