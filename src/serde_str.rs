use std::{
    ops::{Deref, DerefMut},
    str::FromStr,
};

#[derive(Clone, Debug)]
pub(crate) struct Serde<T> {
    inner: T,
}

impl<T> Serde<T> {
    pub(crate) fn new(inner: T) -> Self {
        Serde { inner }
    }

    pub(crate) fn into_inner(this: Self) -> T {
        this.inner
    }
}

impl<T> AsRef<T> for Serde<T> {
    fn as_ref(&self) -> &T {
        &self.inner
    }
}

impl<T> AsMut<T> for Serde<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T> Deref for Serde<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for Serde<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T> FromStr for Serde<T>
where
    T: FromStr,
{
    type Err = T::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Serde {
            inner: T::from_str(s)?,
        })
    }
}

impl<T> serde::Serialize for Serde<T>
where
    T: std::fmt::Display,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = self.inner.to_string();
        serde::Serialize::serialize(s.as_str(), serializer)
    }
}

impl<'de, T> serde::Deserialize<'de> for Serde<T>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = serde::Deserialize::deserialize(deserializer)?;
        let inner = s
            .parse::<T>()
            .map_err(|e| serde::de::Error::custom(e.to_string()))?;

        Ok(Serde { inner })
    }
}
