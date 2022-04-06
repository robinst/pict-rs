use std::collections::HashSet;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Bucket {
    // each Vec<u8> represents a unique image hash
    inner: HashSet<Vec<u8>>,
}

impl Bucket {
    pub(super) fn empty() -> Self {
        Self {
            inner: HashSet::new(),
        }
    }

    pub(super) fn insert(&mut self, alias_bytes: Vec<u8>) {
        self.inner.insert(alias_bytes);
    }

    pub(super) fn remove(&mut self, alias_bytes: &[u8]) {
        self.inner.remove(alias_bytes);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl IntoIterator for Bucket {
    type Item = <HashSet<Vec<u8>> as IntoIterator>::Item;
    type IntoIter = <HashSet<Vec<u8>> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}
