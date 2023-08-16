use crate::store::{object_store::ObjectError, Identifier, StoreError};

#[derive(Debug, Clone)]
pub(crate) struct ObjectId(String);

impl Identifier for ObjectId {
    fn to_bytes(&self) -> Result<Vec<u8>, StoreError> {
        Ok(self.0.as_bytes().to_vec())
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, StoreError> {
        Ok(ObjectId(
            String::from_utf8(bytes).map_err(ObjectError::from)?,
        ))
    }

    fn from_arc(arc: std::sync::Arc<[u8]>) -> Result<Self, StoreError>
    where
        Self: Sized,
    {
        Self::from_bytes(Vec::from(&arc[..]))
    }

    fn string_repr(&self) -> String {
        self.0.clone()
    }
}

impl ObjectId {
    pub(super) fn from_string(string: String) -> Self {
        ObjectId(string)
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}
