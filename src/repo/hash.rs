use crate::formats::InternalFormat;
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Hash {
    hash: Arc<[u8; 32]>,
    size: u64,
    format: InternalFormat,
}

impl Hash {
    pub(crate) fn new(hash: [u8; 32], size: u64, format: InternalFormat) -> Self {
        Self {
            hash: Arc::new(hash),
            format,
            size,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_value() -> Self {
        Self {
            hash: Arc::new([0u8; 32]),
            format: InternalFormat::Image(crate::formats::ImageFormat::Jxl),
            size: 1234,
        }
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
            .field("hash", &hex::encode(*self.hash))
            .field("format", &self.format)
            .field("size", &self.size)
            .finish()
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
struct SerdeHash {
    hash: String,
    size: u64,
    format: InternalFormat,
}

impl serde::Serialize for Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let hash = hex::encode(&self.hash[..]);

        SerdeHash {
            hash,
            size: self.size,
            format: self.format,
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let SerdeHash { hash, size, format } = SerdeHash::deserialize(deserializer)?;
        let hash = hex::decode(hash)
            .map_err(D::Error::custom)?
            .try_into()
            .map_err(|_| D::Error::custom("Invalid hash size"))?;

        Ok(Hash::new(hash, size, format))
    }
}

#[cfg(test)]
mod tests {
    use super::Hash;

    #[test]
    fn round_trip() {
        let hashes = [
            Hash {
                hash: std::sync::Arc::from([0u8; 32]),
                size: 1234,
                format: crate::formats::InternalFormat::Image(crate::formats::ImageFormat::Jxl),
            },
            Hash {
                hash: std::sync::Arc::from([255u8; 32]),
                size: 1234,
                format: crate::formats::InternalFormat::Animation(
                    crate::formats::AnimationFormat::Avif,
                ),
            },
            Hash {
                hash: std::sync::Arc::from([99u8; 32]),
                size: 1234,
                format: crate::formats::InternalFormat::Video(
                    crate::formats::InternalVideoFormat::Mp4,
                ),
            },
        ];

        for hash in hashes {
            let bytes = hash.to_bytes();
            let new_hash = Hash::from_bytes(&bytes).expect("From bytes");
            let new_bytes = new_hash.to_bytes();

            assert_eq!(hash, new_hash, "Hash mismatch");
            assert_eq!(bytes, new_bytes, "Bytes mismatch");
        }
    }
}
