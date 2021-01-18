use crate::{UploadError, CHUNK_SIZE};
use actix_web::{
    dev::Payload,
    http::{
        header::{ContentRange, ContentRangeSpec},
        HeaderValue,
    },
    web::Bytes,
    FromRequest, HttpRequest,
};
use futures::stream::{once, Once, Stream};
use futures_lite::{AsyncReadExt, AsyncSeekExt};
use std::io;
use std::{
    future::{ready, Ready},
    pin::Pin,
};

#[derive(Debug)]
pub(crate) enum Range {
    RangeStart(u64),
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
            Range::RangeStart(start) => ContentRange(ContentRangeSpec::Bytes {
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

    pub(crate) fn chop_bytes(&self, bytes: Bytes) -> Once<Ready<Result<Bytes, io::Error>>> {
        match self {
            Range::RangeStart(start) => once(ready(Ok(bytes.slice(*start as usize..)))),
            Range::SuffixLength(from_start) => once(ready(Ok(bytes.slice(..*from_start as usize)))),
            Range::Segment(start, end) => {
                once(ready(Ok(bytes.slice(*start as usize..*end as usize))))
            }
        }
    }

    pub(crate) async fn chop_file(
        &self,
        mut file: async_fs::File,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>>>>, io::Error> {
        match self {
            Range::RangeStart(start) => {
                file.seek(io::SeekFrom::Start(*start)).await?;

                Ok(Box::pin(crate::read_to_stream(file)))
            }
            Range::SuffixLength(from_start) => {
                file.seek(io::SeekFrom::Start(0)).await?;

                Ok(Box::pin(read_num_bytes_to_stream(file, *from_start)))
            }
            Range::Segment(start, end) => {
                file.seek(io::SeekFrom::Start(*start)).await?;

                Ok(Box::pin(read_num_bytes_to_stream(
                    file,
                    end.saturating_sub(*start),
                )))
            }
        }
    }
}

impl RangeHeader {
    pub(crate) fn is_bytes(&self) -> bool {
        self.unit == "bytes"
    }

    pub(crate) fn ranges<'a>(&'a self) -> impl Iterator<Item = &'a Range> + 'a {
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
    type Config = ();
    type Error = actix_web::Error;
    type Future = std::future::Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        if let Some(range_head) = req.headers().get("Range") {
            ready(parse_range_header(range_head).map_err(|e| {
                tracing::warn!("Failed to parse range header: {}", e);
                e.into()
            }))
        } else {
            ready(Err(UploadError::ParseReq(
                "Range header missing".to_string(),
            )
            .into()))
        }
    }
}

fn parse_range_header(range_head: &HeaderValue) -> Result<RangeHeader, UploadError> {
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
        .collect::<Result<Vec<Range>, UploadError>>()?;

    Ok(RangeHeader {
        unit: unit.to_owned(),
        ranges,
    })
}

fn parse_range(s: &str) -> Result<Range, UploadError> {
    let dash_pos = s
        .find('-')
        .ok_or_else(|| UploadError::ParseReq("Mailformed Range Bound".to_string()))?;

    let (start, end) = s.split_at(dash_pos);
    let start = start.trim();
    let end = end.trim_start_matches('-').trim();

    if start.is_empty() && end.is_empty() {
        Err(UploadError::ParseReq("Malformed content range".to_string()))
    } else if start.is_empty() {
        let suffix_length = end.parse().map_err(|_| {
            UploadError::ParseReq("Cannot parse suffix length for range header".to_string())
        })?;

        Ok(Range::SuffixLength(suffix_length))
    } else if end.is_empty() {
        let range_start = start.parse().map_err(|_| {
            UploadError::ParseReq("Cannot parse range start for range header".to_string())
        })?;

        Ok(Range::RangeStart(range_start))
    } else {
        let range_start = start.parse().map_err(|_| {
            UploadError::ParseReq("Cannot parse range start for range header".to_string())
        })?;
        let range_end = end.parse().map_err(|_| {
            UploadError::ParseReq("Cannot parse range end for range header".to_string())
        })?;

        if range_start > range_end {
            return Err(UploadError::Range);
        }

        Ok(Range::Segment(range_start, range_end))
    }
}

fn read_num_bytes_to_stream(
    mut file: async_fs::File,
    mut num_bytes: u64,
) -> impl Stream<Item = Result<Bytes, io::Error>> {
    async_stream::stream! {
        let mut buf = Vec::with_capacity((CHUNK_SIZE as u64).min(num_bytes) as usize);

        while {
            buf.clear();
            let mut take = (&mut file).take((CHUNK_SIZE as u64).min(num_bytes));

            let read_bytes_result = take.read_to_end(&mut buf).await;

            let read_bytes = read_bytes_result.as_ref().map(|num| *num).unwrap_or(0);

            yield read_bytes_result.map(|_| Bytes::copy_from_slice(&buf));

            num_bytes = num_bytes.saturating_sub(read_bytes as u64);
            read_bytes > 0 && num_bytes > 0
        } {}
    }
}
