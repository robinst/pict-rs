use crate::store::{object_store::ObjectError, Identifier};

#[derive(Debug, Clone)]
pub(crate) struct ObjectId(String);

impl Identifier for ObjectId {
    type Error = ObjectError;

    fn to_bytes(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.0.as_bytes().to_vec())
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        Ok(ObjectId(String::from_utf8(bytes)?))
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
