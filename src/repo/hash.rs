use crate::formats::InternalFormat;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Hash {
    hash: Arc<[u8; 32]>,
    size: u64,
    format: InternalFormat,
}

impl Hash {
    pub(crate) fn new(hash: Arc<[u8; 32]>, size: u64, format: InternalFormat) -> Self {
        Self { hash, format, size }
    }

    pub(super) fn to_bytes(&self) -> Vec<u8> {
        let format = self.format.to_bytes();

        let mut vec = Vec::with_capacity(32 + 8 + format.len());

        vec.extend_from_slice(&self.hash[..]);
        vec.extend(self.size.to_be_bytes());
        vec.extend(format);

        vec
    }

    pub(super) fn to_ivec(&self) -> sled::IVec {
        sled::IVec::from(self.to_bytes())
    }

    pub(super) fn from_ivec(ivec: sled::IVec) -> Option<Self> {
        Self::from_bytes(&ivec)
    }

    pub(super) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 + 8 + 5 {
            return None;
        }

        let hash = &bytes[..32];
        let size = &bytes[32..40];
        let format = &bytes[40..];

        let hash: [u8; 32] = hash.try_into().expect("Correct length");
        let size: [u8; 8] = size.try_into().expect("Correct length");
        let format = InternalFormat::from_bytes(format)?;

        Some(Self {
            hash: Arc::new(hash),
            size: u64::from_be_bytes(size),
            format,
        })
    }
}

impl std::fmt::Debug for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Hash")
            .field("hash", &hex::encode(&*self.hash))
            .field("format", &self.format)
            .field("size", &self.size)
            .finish()
    }
}
