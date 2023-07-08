// TREE STRUCTURE
// - Alias Tree
//   - alias -> hash
//   - alias / id -> u64(id)
//   - alias / delete -> delete token
// - Main Tree
//   - hash -> filename
//   - hash 0 u64(id) -> alias
// - Filename Tree
//   - filename -> hash
// - Details Tree
//   - filename / S::Identifier -> details
// - Identifier Tree
//   - filename -> S::Identifier
//   - filename / variant path -> S::Identifier
//   - filename / motion -> S::Identifier
// - Settings Tree
//   - store-migration-progress -> Path Tree Key

use super::{Alias, DeleteToken, Details};
use std::path::PathBuf;

mod migrate;

#[derive(Debug)]
struct OldDbError(&'static str);

impl std::fmt::Display for OldDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for OldDbError {}

pub(super) struct Old {
    alias_tree: ::sled::Tree,
    filename_tree: ::sled::Tree,
    main_tree: ::sled::Tree,
    details_tree: ::sled::Tree,
    settings_tree: ::sled::Tree,
    identifier_tree: ::sled::Tree,
    _db: ::sled::Db,
}

impl Old {
    #[tracing::instrument]
    pub(super) fn open(path: PathBuf) -> color_eyre::Result<Option<Self>> {
        if let Some(db) = migrate::LatestDb::exists(path).migrate()? {
            Ok(Some(Self {
                alias_tree: db.open_tree("alias")?,
                filename_tree: db.open_tree("filename")?,
                main_tree: db.open_tree("main")?,
                details_tree: db.open_tree("details")?,
                settings_tree: db.open_tree("settings")?,
                identifier_tree: db.open_tree("path")?,
                _db: db,
            }))
        } else {
            Ok(None)
        }
    }

    pub(super) fn setting(&self, key: &[u8]) -> color_eyre::Result<Option<sled::IVec>> {
        Ok(self.settings_tree.get(key)?)
    }

    pub(super) fn hashes(&self) -> impl std::iter::Iterator<Item = sled::IVec> {
        let length = self.filename_tree.len();
        tracing::info!("FILENAME_TREE_LEN: {}", length);
        self.filename_tree
            .iter()
            .values()
            .filter_map(|res| res.ok())
    }

    pub(super) fn details(
        &self,
        hash: &sled::IVec,
    ) -> color_eyre::Result<Vec<(sled::IVec, Details)>> {
        let filename = self
            .main_tree
            .get(hash)?
            .ok_or(OldDbError("Missing filename"))?;

        let filename = String::from_utf8_lossy(&filename);

        Ok(self
            .identifier_tree
            .scan_prefix(filename.as_bytes())
            .values()
            .filter_map(Result::ok)
            .filter_map(|identifier| {
                let mut key = filename.as_bytes().to_vec();
                key.push(b'/');
                key.extend_from_slice(&identifier);

                let details = self.details_tree.get(key).ok()??;
                let details = serde_json::from_slice(&details).ok()?;

                Some((identifier, details))
            })
            .collect())
    }

    pub(super) fn main_identifier(&self, hash: &sled::IVec) -> color_eyre::Result<sled::IVec> {
        let filename = self
            .main_tree
            .get(hash)?
            .ok_or(OldDbError("Missing filename"))?;

        Ok(self
            .identifier_tree
            .get(filename)?
            .ok_or(OldDbError("Missing identifier"))?)
    }

    pub(super) fn variants(
        &self,
        hash: &sled::IVec,
    ) -> color_eyre::Result<Vec<(PathBuf, sled::IVec)>> {
        let filename = self
            .main_tree
            .get(hash)?
            .ok_or(OldDbError("Missing filename"))?;

        let filename_string = String::from_utf8_lossy(&filename);

        let variant_prefix = format!("{filename_string}/");

        Ok(self
            .identifier_tree
            .scan_prefix(&variant_prefix)
            .filter_map(|res| res.ok())
            .filter_map(|(key, value)| {
                let variant_path_bytes = &key[variant_prefix.as_bytes().len()..];
                if variant_path_bytes == b"motion" {
                    return None;
                }

                let path = String::from_utf8(variant_path_bytes.to_vec()).ok()?;
                let mut path = PathBuf::from(path);
                let extension = path.extension()?.to_str()?.to_string();
                path.pop();
                path.push(extension);

                Some((path, value))
            })
            .collect())
    }

    pub(super) fn motion_identifier(
        &self,
        hash: &sled::IVec,
    ) -> color_eyre::Result<Option<sled::IVec>> {
        let filename = self
            .main_tree
            .get(hash)?
            .ok_or(OldDbError("Missing filename"))?;

        let filename_string = String::from_utf8_lossy(&filename);

        let motion_key = format!("{filename_string}/motion");

        Ok(self.filename_tree.get(motion_key)?)
    }

    pub(super) fn aliases(&self, hash: &sled::IVec) -> Vec<Alias> {
        let mut key = hash.to_vec();
        key.push(0);

        self.main_tree
            .scan_prefix(key)
            .values()
            .filter_map(|res| res.ok())
            .filter_map(|alias| Alias::from_slice(&alias))
            .collect()
    }

    pub(super) fn delete_token(&self, alias: &Alias) -> color_eyre::Result<Option<DeleteToken>> {
        let key = format!("{alias}/delete");

        if let Some(ivec) = self.alias_tree.get(key)? {
            return Ok(DeleteToken::from_slice(&ivec));
        }

        Ok(None)
    }
}
