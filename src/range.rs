use crate::{
    error::{Error, UploadError},
    store::Store,
};
use actix_web::{
    dev::Payload,
    http::{
        header::{ContentRange, ContentRangeSpec},
        HeaderValue,
    },
    web::Bytes,
    FromRequest, HttpRequest,
};
use futures_util::stream::{once, Stream};
use std::future::ready;

#[derive(Debug)]
pub(crate) enum Range {
    Start(u64),
    SuffixLength(u64),
    Segment(u64, u64),
}

#[derive(Debug)]
pub(crate) struct RangeHeader {
    unit: String,
    ranges: Vec<Range>,
}

impl Range {
    pub(crate) fn to_content_range(&self, instance_length: u64) -> ContentRange {
        match self {
            Range::Start(start) => ContentRange(ContentRangeSpec::Bytes {
                range: Some((*start, instance_length)),
                instance_length: Some(instance_length),
            }),
            Range::SuffixLength(from_start) => ContentRange(ContentRangeSpec::Bytes {
                range: Some((0, *from_start)),
                instance_length: Some(instance_length),
            }),
            Range::Segment(start, end) => ContentRange(ContentRangeSpec::Bytes {
                range: Some((*start, *end)),
                instance_length: Some(instance_length),
            }),
        }
    }

    pub(crate) fn chop_bytes(&self, bytes: Bytes) -> impl Stream<Item = Result<Bytes, Error>> {
        match self {
            Range::Start(start) => once(ready(Ok(bytes.slice(*start as usize..)))),
            Range::SuffixLength(from_start) => once(ready(Ok(bytes.slice(..*from_start as usize)))),
            Range::Segment(start, end) => {
                once(ready(Ok(bytes.slice(*start as usize..*end as usize))))
            }
        }
    }

    pub(crate) async fn chop_store<S: Store>(
        &self,
        store: &S,
        identifier: S::Identifier,
    ) -> Result<impl Stream<Item = std::io::Result<Bytes>>, Error>
    where
        Error: From<S::Error>,
    {
        match self {
            Range::Start(start) => Ok(store.to_stream(&identifier, Some(*start), None).await?),
            Range::SuffixLength(from_start) => Ok(store
                .to_stream(&identifier, None, Some(*from_start))
                .await?),
            Range::Segment(start, end) => Ok(store
                .to_stream(&identifier, Some(*start), Some(end.saturating_sub(*start)))
                .await?),
        }
    }
}

impl RangeHeader {
    pub(crate) fn is_bytes(&self) -> bool {
        self.unit == "bytes"
    }

    pub(crate) fn ranges(&self) -> impl Iterator<Item = &'_ Range> + '_ {
        self.ranges.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.ranges.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}

impl FromRequest for RangeHeader {
    type Error = Error;
    type Future = std::future::Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        if let Some(range_head) = req.headers().get("Range") {
            ready(parse_range_header(range_head).map_err(|e| {
                tracing::warn!("Failed to parse range header: {}", e);
                e
            }))
        } else {
            ready(Err(UploadError::ParseReq(
                "Range header missing".to_string(),
            )
            .into()))
        }
    }
}

fn parse_range_header(range_head: &HeaderValue) -> Result<RangeHeader, Error> {
    let range_head_str = range_head.to_str().map_err(|_| {
        UploadError::ParseReq("Range header contains non-utf8 characters".to_string())
    })?;

    let eq_pos = range_head_str
        .find('=')
        .ok_or_else(|| UploadError::ParseReq("Malformed Range Header".to_string()))?;

    let (unit, ranges) = range_head_str.split_at(eq_pos);
    let ranges = ranges.trim_start_matches('=');

    let ranges = ranges
        .split(',')
        .map(parse_range)
        .collect::<Result<Vec<Range>, Error>>()?;

    Ok(RangeHeader {
        unit: unit.to_owned(),
        ranges,
    })
}

fn parse_range(s: &str) -> Result<Range, Error> {
    let dash_pos = s
        .find('-')
        .ok_or_else(|| UploadError::ParseReq("Mailformed Range Bound".to_string()))?;

    let (start, end) = s.split_at(dash_pos);
    let start = start.trim();
    let end = end.trim_start_matches('-').trim();

    if start.is_empty() && end.is_empty() {
        Err(UploadError::ParseReq("Malformed content range".to_string()).into())
    } else if start.is_empty() {
        let suffix_length = end.parse().map_err(|_| {
            UploadError::ParseReq("Cannot parse suffix length for range header".to_string())
        })?;

        Ok(Range::SuffixLength(suffix_length))
    } else if end.is_empty() {
        let range_start = start.parse().map_err(|_| {
            UploadError::ParseReq("Cannot parse range start for range header".to_string())
        })?;

        Ok(Range::Start(range_start))
    } else {
        let range_start = start.parse().map_err(|_| {
            UploadError::ParseReq("Cannot parse range start for range header".to_string())
        })?;
        let range_end = end.parse().map_err(|_| {
            UploadError::ParseReq("Cannot parse range end for range header".to_string())
        })?;

        if range_start > range_end {
            return Err(UploadError::Range.into());
        }

        Ok(Range::Segment(range_start, range_end))
    }
}
