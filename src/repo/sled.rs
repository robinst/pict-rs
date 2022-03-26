use super::{
    Alias, AliasRepo, AlreadyExists, DeleteToken, Details, HashRepo, Identifier, IdentifierRepo,
    SettingsRepo,
};
use sled::{Db, IVec, Tree};

macro_rules! b {
    ($self:ident.$ident:ident, $expr:expr) => {{
        let $ident = $self.$ident.clone();

        actix_rt::task::spawn_blocking(move || $expr).await??
    }};
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Error in database")]
    Sled(#[from] sled::Error),

    #[error("Invalid identifier")]
    Identifier(#[source] Box<dyn std::error::Error + Sync + Send>),

    #[error("Invalid details json")]
    Details(#[from] serde_json::Error),

    #[error("Required field was not present")]
    Missing,

    #[error("Operation panicked")]
    Panic,
}

#[derive(Clone)]
pub(crate) struct SledRepo {
    settings: Tree,
    identifier_details: Tree,
    hashes: Tree,
    hash_aliases: Tree,
    hash_identifiers: Tree,
    hash_variant_identifiers: Tree,
    hash_motion_identifiers: Tree,
    aliases: Tree,
    alias_hashes: Tree,
    alias_delete_tokens: Tree,
    _db: Db,
}

impl SledRepo {
    pub(crate) fn new(db: Db) -> Result<Self, Error> {
        Ok(SledRepo {
            settings: db.open_tree("pict-rs-settings-tree")?,
            identifier_details: db.open_tree("pict-rs-identifier-details-tree")?,
            hashes: db.open_tree("pict-rs-hashes-tree")?,
            hash_aliases: db.open_tree("pict-rs-hash-aliases-tree")?,
            hash_identifiers: db.open_tree("pict-rs-hash-identifiers-tree")?,
            hash_variant_identifiers: db.open_tree("pict-rs-hash-variant-identifiers-tree")?,
            hash_motion_identifiers: db.open_tree("pict-rs-hash-motion-identifiers-tree")?,
            aliases: db.open_tree("pict-rs-aliases-tree")?,
            alias_hashes: db.open_tree("pict-rs-alias-hashes-tree")?,
            alias_delete_tokens: db.open_tree("pict-rs-alias-delete-tokens-tree")?,
            _db: db,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl SettingsRepo for SledRepo {
    type Bytes = IVec;
    type Error = Error;

    async fn set(&self, key: &'static [u8], value: Self::Bytes) -> Result<(), Self::Error> {
        b!(self.settings, settings.insert(key, value));

        Ok(())
    }

    async fn get(&self, key: &'static [u8]) -> Result<Option<Self::Bytes>, Self::Error> {
        let opt = b!(self.settings, settings.get(key));

        Ok(opt)
    }

    async fn remove(&self, key: &'static [u8]) -> Result<(), Self::Error> {
        b!(self.settings, settings.remove(key));

        Ok(())
    }
}

fn identifier_bytes<I>(identifier: &I) -> Result<Vec<u8>, Error>
where
    I: Identifier,
{
    identifier
        .to_bytes()
        .map_err(|e| Error::Identifier(Box::new(e)))
}

fn variant_key(hash: &[u8], variant: &str) -> Vec<u8> {
    let mut bytes = hash.to_vec();
    bytes.push(b'/');
    bytes.extend_from_slice(variant.as_bytes());
    bytes
}

fn variant_from_key(hash: &[u8], key: &[u8]) -> Option<String> {
    let prefix_len = hash.len() + 1;
    let variant_bytes = key.get(prefix_len..)?.to_vec();
    String::from_utf8(variant_bytes).ok()
}

#[async_trait::async_trait(?Send)]
impl IdentifierRepo for SledRepo {
    type Error = Error;

    async fn relate_details<I: Identifier>(
        &self,
        identifier: &I,
        details: &Details,
    ) -> Result<(), Self::Error> {
        let key = identifier_bytes(identifier)?;
        let details = serde_json::to_vec(&details)?;

        b!(
            self.identifier_details,
            identifier_details.insert(key, details)
        );

        Ok(())
    }

    async fn details<I: Identifier>(&self, identifier: &I) -> Result<Option<Details>, Self::Error> {
        let key = identifier_bytes(identifier)?;

        let opt = b!(self.identifier_details, identifier_details.get(key));

        if let Some(ivec) = opt {
            Ok(Some(serde_json::from_slice(&ivec)?))
        } else {
            Ok(None)
        }
    }

    async fn cleanup<I: Identifier>(&self, identifier: &I) -> Result<(), Self::Error> {
        let key = identifier_bytes(identifier)?;

        b!(self.identifier_details, identifier_details.remove(key));

        Ok(())
    }
}

type BoxIterator<'a, T> = Box<dyn std::iter::Iterator<Item = T> + Send + 'a>;

type HashIterator = BoxIterator<'static, Result<IVec, sled::Error>>;

type StreamItem = Result<IVec, Error>;

type NextFutResult = Result<(HashIterator, Option<StreamItem>), Error>;

pub(crate) struct HashStream {
    hashes: Option<HashIterator>,
    next_fut: Option<futures_util::future::LocalBoxFuture<'static, NextFutResult>>,
}

impl futures_util::Stream for HashStream {
    type Item = StreamItem;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(mut fut) = this.next_fut.take() {
            match fut.as_mut().poll(cx) {
                std::task::Poll::Ready(Ok((iter, opt))) => {
                    this.hashes = Some(iter);
                    std::task::Poll::Ready(opt)
                }
                std::task::Poll::Ready(Err(e)) => std::task::Poll::Ready(Some(Err(e))),
                std::task::Poll::Pending => {
                    this.next_fut = Some(fut);
                    std::task::Poll::Pending
                }
            }
        } else if let Some(mut iter) = this.hashes.take() {
            let fut = Box::pin(async move {
                actix_rt::task::spawn_blocking(move || {
                    let opt = iter.next();

                    (iter, opt)
                })
                .await
                .map(|(iter, opt)| (iter, opt.map(|res| res.map_err(Error::from))))
                .map_err(Error::from)
            });

            this.next_fut = Some(fut);
            std::pin::Pin::new(this).poll_next(cx)
        } else {
            std::task::Poll::Ready(None)
        }
    }
}

fn hash_alias_key(hash: &IVec, alias: &Alias) -> Vec<u8> {
    let mut v = hash.to_vec();
    v.append(&mut alias.to_bytes());
    v
}

#[async_trait::async_trait(?Send)]
impl HashRepo for SledRepo {
    type Bytes = IVec;
    type Error = Error;
    type Stream = HashStream;

    async fn hashes(&self) -> Self::Stream {
        let iter = self.hashes.iter().keys();

        HashStream {
            hashes: Some(Box::new(iter)),
            next_fut: None,
        }
    }

    async fn create(&self, hash: Self::Bytes) -> Result<Result<(), AlreadyExists>, Self::Error> {
        let res = b!(self.hashes, {
            let hash2 = hash.clone();
            hashes.compare_and_swap(hash, None as Option<Self::Bytes>, Some(hash2))
        });

        Ok(res.map_err(|_| AlreadyExists))
    }

    async fn relate_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), Self::Error> {
        let key = hash_alias_key(&hash, alias);
        let value = alias.to_bytes();

        b!(self.hash_aliases, hash_aliases.insert(key, value));

        Ok(())
    }

    async fn remove_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), Self::Error> {
        let key = hash_alias_key(&hash, alias);

        b!(self.hash_aliases, hash_aliases.remove(key));

        Ok(())
    }

    async fn aliases(&self, hash: Self::Bytes) -> Result<Vec<Alias>, Self::Error> {
        let v = b!(self.hash_aliases, {
            Ok(hash_aliases
                .scan_prefix(hash)
                .values()
                .filter_map(Result::ok)
                .filter_map(|ivec| Alias::from_slice(&ivec))
                .collect::<Vec<_>>()) as Result<_, sled::Error>
        });

        Ok(v)
    }

    async fn relate_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), Self::Error> {
        let bytes = identifier_bytes(identifier)?;

        b!(self.hash_identifiers, hash_identifiers.insert(hash, bytes));

        Ok(())
    }

    async fn identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<I, Self::Error> {
        let opt = b!(self.hash_identifiers, hash_identifiers.get(hash));

        opt.ok_or(Error::Missing).and_then(|ivec| {
            I::from_bytes(ivec.to_vec()).map_err(|e| Error::Identifier(Box::new(e)))
        })
    }

    async fn relate_variant_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        variant: String,
        identifier: &I,
    ) -> Result<(), Self::Error> {
        let key = variant_key(&hash, &variant);
        let value = identifier_bytes(identifier)?;

        b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.insert(key, value)
        );

        Ok(())
    }

    async fn variant_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
        variant: String,
    ) -> Result<Option<I>, Self::Error> {
        let key = variant_key(&hash, &variant);

        let opt = b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.get(key)
        );

        if let Some(ivec) = opt {
            Ok(Some(
                I::from_bytes(ivec.to_vec()).map_err(|e| Error::Identifier(Box::new(e)))?,
            ))
        } else {
            Ok(None)
        }
    }

    async fn variants<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Vec<(String, I)>, Self::Error> {
        let vec = b!(
            self.hash_variant_identifiers,
            Ok(hash_variant_identifiers
                .scan_prefix(&hash)
                .filter_map(|res| res.ok())
                .filter_map(|(key, ivec)| {
                    let identifier = I::from_bytes(ivec.to_vec()).ok()?;
                    let variant = variant_from_key(&hash, &key)?;

                    Some((variant, identifier))
                })
                .collect::<Vec<_>>()) as Result<Vec<_>, sled::Error>
        );

        Ok(vec)
    }

    async fn relate_motion_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), Self::Error> {
        let bytes = identifier_bytes(identifier)?;

        b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.insert(hash, bytes)
        );

        Ok(())
    }

    async fn motion_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, Self::Error> {
        let opt = b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.get(hash)
        );

        if let Some(ivec) = opt {
            Ok(Some(
                I::from_bytes(ivec.to_vec()).map_err(|e| Error::Identifier(Box::new(e)))?,
            ))
        } else {
            Ok(None)
        }
    }

    async fn cleanup(&self, hash: Self::Bytes) -> Result<(), Self::Error> {
        let hash2 = hash.clone();
        b!(self.hashes, hashes.remove(hash2));

        let hash2 = hash.clone();
        b!(self.hash_identifiers, hash_identifiers.remove(hash2));

        let hash2 = hash.clone();
        b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.remove(hash2)
        );

        let aliases = self.aliases(hash.clone()).await?;
        let hash2 = hash.clone();
        b!(self.hash_aliases, {
            for alias in aliases {
                let key = hash_alias_key(&hash2, &alias);

                let _ = hash_aliases.remove(key);
            }
            Ok(()) as Result<(), sled::Error>
        });

        let variant_keys = b!(self.hash_variant_identifiers, {
            let v = hash_variant_identifiers
                .scan_prefix(hash)
                .keys()
                .filter_map(Result::ok)
                .collect::<Vec<_>>();

            Ok(v) as Result<Vec<_>, sled::Error>
        });
        b!(self.hash_variant_identifiers, {
            for key in variant_keys {
                let _ = hash_variant_identifiers.remove(key);
            }
            Ok(()) as Result<(), sled::Error>
        });

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl AliasRepo for SledRepo {
    type Bytes = sled::IVec;
    type Error = Error;

    async fn create(&self, alias: &Alias) -> Result<Result<(), AlreadyExists>, Self::Error> {
        let bytes = alias.to_bytes();
        let bytes2 = bytes.clone();

        let res = b!(
            self.aliases,
            aliases.compare_and_swap(bytes, None as Option<Self::Bytes>, Some(bytes2))
        );

        Ok(res.map_err(|_| AlreadyExists))
    }

    async fn relate_delete_token(
        &self,
        alias: &Alias,
        delete_token: &DeleteToken,
    ) -> Result<Result<(), AlreadyExists>, Self::Error> {
        let key = alias.to_bytes();
        let token = delete_token.to_bytes();

        let res = b!(
            self.alias_delete_tokens,
            alias_delete_tokens.compare_and_swap(key, None as Option<Self::Bytes>, Some(token))
        );

        Ok(res.map_err(|_| AlreadyExists))
    }

    async fn delete_token(&self, alias: &Alias) -> Result<DeleteToken, Self::Error> {
        let key = alias.to_bytes();

        let opt = b!(self.alias_delete_tokens, alias_delete_tokens.get(key));

        opt.and_then(|ivec| DeleteToken::from_slice(&ivec))
            .ok_or(Error::Missing)
    }

    async fn relate_hash(&self, alias: &Alias, hash: Self::Bytes) -> Result<(), Self::Error> {
        let key = alias.to_bytes();

        b!(self.alias_hashes, alias_hashes.insert(key, hash));

        Ok(())
    }

    async fn hash(&self, alias: &Alias) -> Result<Self::Bytes, Self::Error> {
        let key = alias.to_bytes();

        let opt = b!(self.alias_hashes, alias_hashes.get(key));

        opt.ok_or(Error::Missing)
    }

    async fn cleanup(&self, alias: &Alias) -> Result<(), Self::Error> {
        let key = alias.to_bytes();

        let key2 = key.clone();
        b!(self.aliases, aliases.remove(key2));

        let key2 = key.clone();
        b!(self.alias_delete_tokens, alias_delete_tokens.remove(key2));

        b!(self.alias_hashes, alias_hashes.remove(key));

        Ok(())
    }
}

impl std::fmt::Debug for SledRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SledRepo").finish()
    }
}

impl From<actix_rt::task::JoinError> for Error {
    fn from(_: actix_rt::task::JoinError) -> Self {
        Error::Panic
    }
}
