use crate::{
    error::Error,
    store::{
        file_store::{FileError, FileStore},
        Identifier,
    },
};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(crate) struct FileId(PathBuf);

impl Identifier for FileId {
    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let vec = self
            .0
            .to_str()
            .ok_or(FileError::IdError)?
            .as_bytes()
            .to_vec();

        Ok(vec)
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let string = String::from_utf8(bytes).map_err(|_| FileError::IdError)?;

        let id = FileId(PathBuf::from(string));

        Ok(id)
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
