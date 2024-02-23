use std::sync::Arc;

use crate::{
    error::{Error, UploadError},
    store::Store,
};
use actix_web::{
    http::header::{ByteRangeSpec, ContentRange, ContentRangeSpec, Range},
    web::Bytes,
};
use futures_core::Stream;

pub(crate) async fn chop_store<S: Store>(
    byte_range: &ByteRangeSpec,
    store: &S,
    identifier: &Arc<str>,
    length: u64,
) -> Result<impl Stream<Item = std::io::Result<Bytes>>, Error> {
    if let Some((start, end)) = byte_range.to_satisfiable_range(length) {
        // END IS INCLUSIVE
        let end = end + 1;
        return store
            .to_stream(identifier, Some(start), Some(end.saturating_sub(start)))
            .await
            .map_err(Error::from);
    }

    Err(UploadError::Range.into())
}

pub(crate) fn single_bytes_range(range: &Range) -> Option<&ByteRangeSpec> {
    if let Range::Bytes(ranges) = range {
        if ranges.len() == 1 {
            return ranges.first();
        }
    }

    None
}

pub(crate) fn to_content_range(
    byte_range: &ByteRangeSpec,
    instance_length: u64,
) -> Option<ContentRange> {
    byte_range
        .to_satisfiable_range(instance_length)
        .map(|range| {
            ContentRange(ContentRangeSpec::Bytes {
                range: Some(range),
                instance_length: Some(instance_length),
            })
        })
}
