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
use uuid::Uuid;

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
    pub(super) fn open(db: sled::Db) -> anyhow::Result<Self> {
        Ok(Self {
            alias_tree: db.open_tree("alias")?,
            filename_tree: db.open_tree("filename")?,
            main_tree: db.open_tree("main")?,
            details_tree: db.open_tree("details")?,
            settings_tree: db.open_tree("settings")?,
            identifier_tree: db.open_tree("path")?,
            _db: db,
        })
    }

    pub(super) fn setting(&self, key: &[u8]) -> anyhow::Result<Option<sled::IVec>> {
        Ok(self.settings_tree.get(key)?)
    }

    pub(super) fn hashes(&self) -> impl std::iter::Iterator<Item = sled::IVec> {
        self.filename_tree
            .iter()
            .values()
            .filter_map(|res| res.ok())
    }

    pub(super) fn details(&self, hash: &sled::IVec) -> anyhow::Result<Vec<(sled::IVec, Details)>> {
        let filename = self
            .main_tree
            .get(hash)?
            .ok_or_else(|| anyhow::anyhow!("missing filename"))?;

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

    pub(super) fn main_identifier(&self, hash: &sled::IVec) -> anyhow::Result<sled::IVec> {
        let filename = self
            .main_tree
            .get(hash)?
            .ok_or_else(|| anyhow::anyhow!("Missing filename"))?;

        self.filename_tree
            .get(filename)?
            .ok_or_else(|| anyhow::anyhow!("Missing identifier"))
    }

    pub(super) fn variants(&self, hash: &sled::IVec) -> anyhow::Result<Vec<(String, sled::IVec)>> {
        let filename = self
            .main_tree
            .get(hash)?
            .ok_or_else(|| anyhow::anyhow!("Missing filename"))?;

        let filename_string = String::from_utf8_lossy(&filename);

        let variant_prefix = format!("{}/", filename_string);

        Ok(self
            .identifier_tree
            .scan_prefix(&variant_prefix)
            .filter_map(|res| res.ok())
            .filter_map(|(key, value)| {
                let key_str = String::from_utf8_lossy(&key);
                let variant_path = key_str.trim_start_matches(&variant_prefix);
                if variant_path == "motion" {
                    return None;
                }

                Some((variant_path.to_string(), value))
            })
            .collect())
    }

    pub(super) fn motion_identifier(
        &self,
        hash: &sled::IVec,
    ) -> anyhow::Result<Option<sled::IVec>> {
        let filename = self
            .main_tree
            .get(hash)?
            .ok_or_else(|| anyhow::anyhow!("Missing filename"))?;

        let filename_string = String::from_utf8_lossy(&filename);

        let motion_key = format!("{}/motion", filename_string);

        Ok(self.filename_tree.get(motion_key)?)
    }

    pub(super) fn aliases(&self, hash: &sled::IVec) -> Vec<Alias> {
        let mut key = hash.to_vec();
        key.push(0);

        self.main_tree
            .scan_prefix(key)
            .values()
            .filter_map(|res| res.ok())
            .filter_map(|alias| {
                let alias_str = String::from_utf8_lossy(&alias);

                let (uuid, ext) = alias_str.split_once('.')?;

                let uuid = uuid.parse::<Uuid>().ok()?;

                Some(Alias {
                    id: uuid,
                    extension: ext.to_string(),
                })
            })
            .collect()
    }

    pub(super) fn delete_token(&self, alias: &Alias) -> anyhow::Result<Option<DeleteToken>> {
        let key = format!("{}{}/delete", alias.id, alias.extension);

        if let Some(ivec) = self.alias_tree.get(key)? {
            let token_str = String::from_utf8_lossy(&ivec);
            if let Ok(uuid) = token_str.parse::<Uuid>() {
                return Ok(Some(DeleteToken { id: uuid }));
            }
        }

        Ok(None)
    }
}
