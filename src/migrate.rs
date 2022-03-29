use crate::Error;
use std::path::PathBuf;

mod s034;

type SledIter = Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), Error>>>;

trait SledDb {
    type SledTree: SledTree;

    fn open_tree(&self, name: &str) -> Result<Self::SledTree, Error>;

    fn self_tree(&self) -> &Self::SledTree;
}

impl<T> SledDb for &T
where
    T: SledDb,
{
    type SledTree = T::SledTree;

    fn open_tree(&self, name: &str) -> Result<Self::SledTree, Error> {
        (*self).open_tree(name)
    }

    fn self_tree(&self) -> &Self::SledTree {
        (*self).self_tree()
    }
}

trait SledTree {
    fn get<K>(&self, key: K) -> Result<Option<Vec<u8>>, Error>
    where
        K: AsRef<[u8]>;

    fn insert<K, V>(&self, key: K, value: V) -> Result<(), Error>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>;

    fn iter(&self) -> SledIter;

    fn range<K, R>(&self, range: R) -> SledIter
    where
        K: AsRef<[u8]>,
        R: std::ops::RangeBounds<K>;

    fn flush(&self) -> Result<(), Error>;
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

    pub(crate) fn migrate(self) -> Result<sled::Db, Error> {
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

    fn migrate(self, root: PathBuf) -> Result<sled::Db, Error> {
        match self {
            DbVersion::Sled034 | DbVersion::Fresh => s034::open(root),
        }
    }
}
