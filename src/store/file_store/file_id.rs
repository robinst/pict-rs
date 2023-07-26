use crate::store::{
    file_store::{FileError, FileStore},
    Identifier, StoreError,
};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileId(PathBuf);

impl Identifier for FileId {
    fn to_bytes(&self) -> Result<Vec<u8>, StoreError> {
        let vec = self
            .0
            .to_str()
            .ok_or(FileError::IdError)?
            .as_bytes()
            .to_vec();

        Ok(vec)
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, StoreError>
    where
        Self: Sized,
    {
        let string = String::from_utf8(bytes).map_err(|_| FileError::IdError)?;

        let id = FileId(PathBuf::from(string));

        Ok(id)
    }

    fn string_repr(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl FileStore {
    pub(super) fn file_id_from_path(&self, path: PathBuf) -> Result<FileId, FileError> {
        let stripped = path
            .strip_prefix(&self.root_dir)
            .map_err(|_| FileError::PrefixError)?;

        Ok(FileId(stripped.to_path_buf()))
    }

    pub(super) fn path_from_file_id(&self, file_id: &FileId) -> PathBuf {
        self.root_dir.join(&file_id.0)
    }
}
