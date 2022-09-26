use crate::{
    error::Error,
    store::{object_store::ObjectError, Identifier},
};

#[derive(Debug, Clone)]
pub(crate) struct ObjectId(String);

impl Identifier for ObjectId {
    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        Ok(self.0.as_bytes().to_vec())
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, Error> {
        Ok(ObjectId(
            String::from_utf8(bytes).map_err(ObjectError::from)?,
        ))
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
