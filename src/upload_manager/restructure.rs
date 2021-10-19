use crate::{
    error::{Error, UploadError},
    safe_move_file,
    upload_manager::UploadManager,
};
use std::path::{Path, PathBuf};

const RESTRUCTURE_COMPLETE: &'static [u8] = b"fs-restructure-01-complete";
const DETAILS: &'static [u8] = b"details";

impl UploadManager {
    #[tracing::instrument(skip(self))]
    pub(super) async fn restructure(&self) -> Result<(), Error> {
        if self.restructure_complete()? {
            return Ok(());
        }

        for res in self.inner.filename_tree.iter() {
            let (filename, hash) = res?;
            let filename = String::from_utf8(filename.to_vec())?;
            tracing::info!("Migrating {}", filename);

            let mut file_path = self.image_dir();
            file_path.push(filename.clone());

            if tokio::fs::metadata(&file_path).await.is_ok() {
                let mut target_path = self.next_directory()?;
                target_path.push(filename.clone());

                let target_path_bytes = self
                    .generalize_path(&target_path)?
                    .to_str()
                    .ok_or(UploadError::Path)?
                    .as_bytes()
                    .to_vec();

                self.inner
                    .path_tree
                    .insert(filename.as_bytes(), target_path_bytes)?;

                safe_move_file(file_path, target_path).await?;
            }

            let (start, end) = variant_key_bounds(&hash);

            for res in self.inner.main_tree.range(start..end) {
                let (hash_variant_key, variant_path_or_details) = res?;

                if hash_variant_key.ends_with(DETAILS) {
                    let details = variant_path_or_details;

                    let start_index = hash.len() + 1;
                    let end_index = hash_variant_key.len() - DETAILS.len();
                    let path_bytes = &hash_variant_key[start_index..end_index];

                    let variant_path = PathBuf::from(String::from_utf8(path_bytes.to_vec())?);
                    let key = self.details_key(&variant_path, &filename)?;

                    self.inner.details_tree.insert(key, details)?;
                } else {
                    let variant_path =
                        PathBuf::from(String::from_utf8(variant_path_or_details.to_vec())?);
                    if tokio::fs::metadata(&variant_path).await.is_ok() {
                        let mut target_path = self.next_directory()?;
                        target_path.push(filename.clone());

                        let relative_target_path_bytes = self
                            .generalize_path(&target_path)?
                            .to_str()
                            .ok_or(UploadError::Path)?
                            .as_bytes()
                            .to_vec();

                        let variant_key = self.variant_key(&target_path, &filename)?;

                        self.inner
                            .path_tree
                            .insert(variant_key, relative_target_path_bytes)?;

                        safe_move_file(variant_path, target_path).await?;
                    }
                }

                self.inner.main_tree.remove(hash_variant_key)?;
            }
        }

        self.mark_restructure_complete()?;
        Ok(())
    }

    fn restructure_complete(&self) -> Result<bool, Error> {
        Ok(!self
            .inner
            .settings_tree
            .get(RESTRUCTURE_COMPLETE)?
            .is_some())
    }

    fn mark_restructure_complete(&self) -> Result<(), Error> {
        self.inner
            .settings_tree
            .insert(RESTRUCTURE_COMPLETE, b"true")?;

        Ok(())
    }

    pub(super) fn generalize_path<'a>(&self, path: &'a Path) -> Result<&'a Path, Error> {
        Ok(path.strip_prefix(&self.inner.image_dir)?)
    }

    pub(super) fn details_key(
        &self,
        variant_path: &Path,
        filename: &str,
    ) -> Result<Vec<u8>, Error> {
        let path = self.generalize_path(variant_path)?;
        let path_string = path.to_str().ok_or(UploadError::Path)?.to_string();

        let vec = format!("{}/{}", filename, path_string).as_bytes().to_vec();
        Ok(vec)
    }

    pub(super) fn variant_key(
        &self,
        variant_path: &Path,
        filename: &str,
    ) -> Result<Vec<u8>, Error> {
        let path = self.generalize_path(variant_path)?;
        let path_string = path.to_str().ok_or(UploadError::Path)?.to_string();

        let vec = format!("{}/{}", filename, path_string).as_bytes().to_vec();
        Ok(vec)
    }
}

pub(crate) fn variant_key_bounds(hash: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut start = hash.to_vec();
    start.extend(&[2]);

    let mut end = hash.to_vec();
    end.extend(&[3]);

    (start, end)
}
