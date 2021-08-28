use crate::UploadError;
use actix_web::{
    dev::Payload,
    http::{
        header::{ContentRange, ContentRangeSpec},
        HeaderValue,
    },
    web::Bytes,
    FromRequest, HttpRequest,
};
use futures::stream::{Stream, StreamExt, TryStreamExt};
use std::{fs, io};
use std::{future::ready, pin::Pin};

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

    pub(crate) async fn chop_file(
        &self,
        file: fs::File,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, UploadError>>>>, UploadError> {
        match self {
            Range::RangeStart(start) => {
                let (file, _) = actix_fs::file::seek(file, io::SeekFrom::Start(*start)).await?;

                Ok(Box::pin(
                    actix_fs::file::read_to_stream(file)
                        .await?
                        .faster()
                        .map_err(UploadError::from),
                ))
            }
            Range::SuffixLength(from_start) => {
                let (file, _) = actix_fs::file::seek(file, io::SeekFrom::Start(0)).await?;

                Ok(Box::pin(
                    read_num_bytes_to_stream(file, *from_start as usize).await?,
                ))
            }
            Range::Segment(start, end) => {
                let (file, _) = actix_fs::file::seek(file, io::SeekFrom::Start(*start)).await?;

                Ok(Box::pin(
                    read_num_bytes_to_stream(file, end.saturating_sub(*start) as usize).await?,
                ))
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

async fn read_num_bytes_to_stream(
    file: fs::File,
    mut num_bytes: usize,
) -> Result<impl Stream<Item = Result<Bytes, UploadError>>, UploadError> {
    let mut stream = actix_fs::file::read_to_stream(file).await?;

    let stream = async_stream::stream! {
        while let Some(res) = stream.next().await {
            let read_bytes = res.as_ref().map(|b| b.len()).unwrap_or(0);

            if read_bytes == 0 {
                break;
            }

            yield res.map_err(UploadError::from).map(|bytes| {
                if bytes.len() > num_bytes {
                    bytes.slice(0..num_bytes)
                } else {
                    bytes
                }
            });

            num_bytes = num_bytes.saturating_sub(read_bytes);
        }
    };

    Ok(stream)
}
