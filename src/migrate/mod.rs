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
            let res = std::panic::catch_unwind(move || {
                version.migrate(root_dir2)
            });

            if let Ok(res) = res {
                return res;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum DbVersion {
    Sled0320Rc1,
    Sled032,
    Sled034,
    Fresh,
}

impl DbVersion {
    fn exists(root: PathBuf) -> Self {
        if s034::exists(root.clone()) && !s034::migrating(root.clone()) {
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

    let new_migrate_tree = new_db.open_tree("migrate")?;
    if let Some(_) = new_migrate_tree.get("done")? {
        return Ok(new_db);
    }

    let (iterator, mut counter) = if let Some(last_migrated) = new_migrate_tree.get("last_migrated")? {
        let mut last_migrated = String::from_utf8_lossy(&last_migrated).to_string();
        info!("Previous migration failed after {}, attempting to skip", last_migrated);
        if let Some(index) = last_migrated.find('.') {
            last_migrated = last_migrated.split_at(index).0.to_owned();
        }
        if last_migrated.len() > 3 {
            last_migrated = last_migrated.split_at(3).0.to_owned();
        }
        let last_migrated = increment_alphanumeric(&last_migrated).as_bytes().to_owned();
        new_migrate_tree.insert("last_migrated", last_migrated.clone())?;
        new_migrate_tree.flush()?;
        if let Some(count) = new_migrate_tree.get("counter")? {
            (old_alias_tree.range(last_migrated..), String::from_utf8_lossy(&count).parse::<usize>().unwrap())
        } else {
            (old_alias_tree.range(last_migrated..), 0)
        }
    } else {
        (old_alias_tree.iter(), 0)
    };

    for res in iterator {
        let (k, _) = res?;

        if let Some(v) = old_alias_tree.get(&k)? {
            if !k.contains(&b"/"[0]) {
                if let Some(id) = old_alias_tree.get(alias_id_key(&String::from_utf8_lossy(&k)))? {
                    counter += 1;
                    info!("Migrating alias #{}", counter);
                    // k is an alias
                    migrate_main_tree(&k, &v, &old_db, &new_db, &String::from_utf8_lossy(&id))?;
                    debug!(
                        "Moving alias -> hash for alias {}",
                        String::from_utf8_lossy(k.as_ref()),
                    );
                    new_migrate_tree.insert("counter", format!("{}", counter))?;
                }
            } else {
                debug!(
                    "Moving {}, {}",
                    String::from_utf8_lossy(k.as_ref()),
                    String::from_utf8_lossy(v.as_ref())
                );
            }
            new_alias_tree.insert(k.clone(), v)?;
            new_migrate_tree.insert("last_migrated", k)?;
            new_migrate_tree.flush()?;
        } else {
            warn!("MISSING {}", String::from_utf8_lossy(k.as_ref()));
        }
    }
    info!("Moved {} unique aliases", counter);

    new_migrate_tree.insert("done", "true")?;

    Ok(new_db)
}

fn migrate_main_tree<Old, New>(
    alias: &[u8],
    hash: &[u8],
    old_db: Old,
    new_db: New,
    id: &str,
) -> Result<(), UploadError>
where
    Old: SledDb,
    New: SledDb,
{
    let main_tree = new_db.open_tree("main")?;

    let new_fname_tree = new_db.open_tree("filename")?;

    debug!(
        "Migrating files for {}",
        String::from_utf8_lossy(alias.as_ref())
    );
    if let Some(filename) = old_db.self_tree().get(&hash)? {
        main_tree.insert(&hash, filename.clone())?;
        new_fname_tree.insert(filename, hash.clone())?;

    } else {
        warn!("Missing filename");
    }

    let key = alias_key(&hash, id);
    if let Some(v) = old_db.self_tree().get(&key)? {
        main_tree.insert(key, v)?;
    } else {
        warn!("Not migrating alias {} id {}", String::from_utf8_lossy(&alias), id);
        return Ok(());
    }

    let (start, end) = variant_key_bounds(&hash);
    if main_tree.range(start.clone()..end.clone()).next().is_none() {
        let mut counter = 0;
        for res in old_db.self_tree().range(start.clone()..end.clone()) {
            counter += 1;
            let (k, v) = res?;
            debug!("Moving variant #{} for {}", counter, String::from_utf8_lossy(v.as_ref()));
            main_tree.insert(k, v)?;
        }
        info!("Moved {} variants for {}", counter, String::from_utf8_lossy(alias.as_ref()));
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

const VALID: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
];

fn increment_alphanumeric(input: &str) -> String {
    let (_, output) = input.chars().rev().fold((true, String::new()), |(incr_next, mut acc), item| {
        if incr_next {
            let mut index = None;
            for (i, test) in VALID.iter().enumerate() {
                if *test == item {
                    index = Some(i);
                }
            }
            let index = index.unwrap_or(0);
            let (set_incr_next, next_index) = if index == (VALID.len() - 1) {
                (true, 0)
            } else {
                (false, index + 1)
            };
            acc.extend(&[VALID[next_index]]);
            (set_incr_next, acc)
        } else {
            acc.extend(&[item]);
            (false, acc)
        }
    });

    output.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::increment_alphanumeric;

    #[test]
    fn increments() {
        assert_eq!(increment_alphanumeric("hello"), "hellp");
        assert_eq!(increment_alphanumeric("0"), "1");
        assert_eq!(increment_alphanumeric("9"), "A");
        assert_eq!(increment_alphanumeric("Z"), "a");
        assert_eq!(increment_alphanumeric("z"), "0");
        assert_eq!(increment_alphanumeric("az"), "b0");
        assert_eq!(increment_alphanumeric("19"), "1A");
        assert_eq!(increment_alphanumeric("AZ"), "Aa");
    }
}
