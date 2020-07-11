use ffmpeg_next::{
    self, codec, filter, format, frame, media,
    util::{format::pixel::Pixel, rational::Rational},
};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("No video stream present")]
    MissingVideo,

    #[error("Input format is not supported")]
    UnsupportedFormat,

    #[error("No frame-rate present in input video")]
    FrameRate,

    #[error("Filter {0} should have been set up by now")]
    MissingFilter(&'static str),

    #[error("{0}")]
    Transcode(#[from] ffmpeg_next::Error),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Target {
    Mp4,
    Jpeg,
    #[allow(dead_code)]
    Png,
}

impl Target {
    fn name(&self) -> &'static str {
        match self {
            Target::Mp4 => "mp4",
            Target::Jpeg => "image2",
            Target::Png => "image2",
        }
    }

    fn codec(&self) -> codec::id::Id {
        match self {
            Target::Mp4 => codec::id::Id::H264,
            Target::Jpeg => codec::id::Id::MJPEG,
            Target::Png => codec::id::Id::PNG,
        }
    }

    fn frames(&self) -> Option<usize> {
        match self {
            Target::Mp4 => None,
            Target::Jpeg => Some(1),
            Target::Png => Some(1),
        }
    }
}

fn pixel_value(pixel: Pixel) -> i32 {
    let av_px_fmt: ffmpeg_sys_next::AVPixelFormat = pixel.into();
    unsafe { std::mem::transmute::<_, i32>(av_px_fmt) }
}

fn to_even(num: u32) -> u32 {
    if num % 2 == 0 {
        num
    } else {
        num - 1
    }
}

fn filter(
    decoder: &codec::decoder::Video,
    encoder: &codec::encoder::Video,
) -> Result<filter::Graph, Error> {
    let mut filter = filter::Graph::new();

    let aspect = Rational::new(
        to_even(decoder.width()) as i32,
        to_even(decoder.height()) as i32,
    )
    .reduce();

    let av_px_fmt = pixel_value(decoder.format());

    let args = format!(
        "video_size={width}x{height}:pix_fmt={pix_fmt}:time_base={time_base_num}/{time_base_den}:pixel_aspect={pix_aspect_num}/{pix_aspect_den}",
        width=to_even(decoder.width()),
        height=to_even(decoder.height()),
        pix_fmt=av_px_fmt,
        time_base_num=decoder.time_base().numerator(),
        time_base_den=decoder.time_base().denominator(),
        pix_aspect_num=aspect.numerator(),
        pix_aspect_den=aspect.denominator(),
    );

    let buffer = filter::find("buffer").ok_or(Error::MissingFilter("buffer"))?;
    filter.add(&buffer, "in", &args)?;

    let buffersink = filter::find("buffersink").ok_or(Error::MissingFilter("buffersink"))?;
    let mut out = filter.add(&buffersink, "out", "")?;
    out.set_pixel_format(encoder.format());

    filter.output("in", 0)?.input("out", 0)?.parse("null")?;
    filter.validate()?;

    println!("{}", filter.dump());

    if let Some(codec) = encoder.codec() {
        if !codec
            .capabilities()
            .contains(codec::capabilities::Capabilities::VARIABLE_FRAME_SIZE)
        {
            filter
                .get("out")
                .ok_or(Error::MissingFilter("out"))?
                .sink()
                .set_frame_size(encoder.frame_size());
        }
    }

    Ok(filter)
}

struct Transcoder {
    stream: usize,
    filter: filter::Graph,
    decoder: codec::decoder::Video,
    encoder: codec::encoder::Video,
}

impl Transcoder {
    fn decode(
        &mut self,
        packet: &ffmpeg_next::Packet,
        decoded: &mut frame::Video,
    ) -> Result<bool, ffmpeg_next::Error> {
        self.decoder.decode(packet, decoded)
    }

    fn add_frame(&mut self, decoded: &frame::Video) -> Result<(), Error> {
        self.filter
            .get("in")
            .ok_or(Error::MissingFilter("out"))?
            .source()
            .add(decoded)?;
        Ok(())
    }

    fn encode(
        &mut self,
        decoded: &mut frame::Video,
        encoded: &mut ffmpeg_next::Packet,
        octx: &mut format::context::Output,
        in_time_base: Rational,
        out_time_base: Rational,
    ) -> Result<(), Error> {
        while let Ok(()) = self
            .filter
            .get("out")
            .ok_or(Error::MissingFilter("out"))?
            .sink()
            .frame(decoded)
        {
            if let Ok(true) = self.encoder.encode(decoded, encoded) {
                encoded.set_stream(0);
                encoded.rescale_ts(in_time_base, out_time_base);
                encoded.write_interleaved(octx)?;
            }
        }

        Ok(())
    }

    fn flush(
        &mut self,
        decoded: &mut frame::Video,
        encoded: &mut ffmpeg_next::Packet,
        octx: &mut format::context::Output,
        in_time_base: Rational,
        out_time_base: Rational,
    ) -> Result<(), Error> {
        self.filter
            .get("in")
            .ok_or(Error::MissingFilter("in"))?
            .source()
            .flush()?;

        self.encode(decoded, encoded, octx, in_time_base, out_time_base)?;

        while let Ok(true) = self.encoder.flush(encoded) {
            encoded.set_stream(0);
            encoded.rescale_ts(in_time_base, out_time_base);
            encoded.write_interleaved(octx)?;
        }
        Ok(())
    }
}

fn transcoder(
    ictx: &mut format::context::Input,
    octx: &mut format::context::Output,
    target: Target,
) -> Result<Transcoder, Error> {
    let input = ictx
        .streams()
        .best(media::Type::Video)
        .ok_or(Error::MissingVideo)?;
    let mut decoder = input.codec().decoder().video()?;
    let codec_id = target.codec();
    let codec = ffmpeg_next::encoder::find(codec_id)
        .ok_or(Error::UnsupportedFormat)?
        .video()?;
    let global = octx
        .format()
        .flags()
        .contains(format::flag::Flags::GLOBAL_HEADER);

    decoder.set_parameters(input.parameters())?;

    let mut output = octx.add_stream(codec)?;
    let mut encoder = output.codec().encoder().video()?;

    if global {
        encoder.set_flags(codec::flag::Flags::GLOBAL_HEADER);
    }

    encoder.set_format(
        codec
            .formats()
            .ok_or(Error::UnsupportedFormat)?
            .next()
            .ok_or(Error::UnsupportedFormat)?,
    );
    encoder.set_bit_rate(decoder.bit_rate());
    encoder.set_max_bit_rate(decoder.max_bit_rate());

    encoder.set_width(to_even(decoder.width()));
    encoder.set_height(to_even(decoder.height()));
    encoder.set_bit_rate(decoder.bit_rate());
    encoder.set_max_bit_rate(decoder.max_bit_rate());
    encoder.set_time_base(decoder.frame_rate().ok_or(Error::FrameRate)?.invert());
    output.set_time_base(decoder.time_base());

    let encoder = encoder.open_as(codec)?;
    output.set_parameters(&encoder);

    let filter = filter(&decoder, &encoder)?;

    Ok(Transcoder {
        stream: input.index(),
        filter: filter,
        decoder: decoder,
        encoder: encoder,
    })
}

pub(crate) fn transcode<P, Q>(input: P, output: Q, target: Target) -> Result<(), Error>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    let mut ictx = format::input(&input)?;
    let mut octx = format::output_as(&output, target.name())?;
    let mut transcoder = transcoder(&mut ictx, &mut octx, target)?;

    octx.write_header()?;

    let in_time_base = transcoder.decoder.time_base();
    let out_time_base = octx.stream(0).ok_or(Error::MissingVideo)?.time_base();

    let mut decoded = frame::Video::empty();
    let mut encoded = ffmpeg_next::Packet::empty();
    let mut count = 0;

    for (stream, mut packet) in ictx.packets() {
        if stream.index() == transcoder.stream {
            packet.rescale_ts(stream.time_base(), in_time_base);

            if let Ok(true) = transcoder.decode(&packet, &mut decoded) {
                let timestamp = decoded.timestamp();
                decoded.set_pts(timestamp);

                transcoder.add_frame(&decoded)?;

                transcoder.encode(
                    &mut decoded,
                    &mut encoded,
                    &mut octx,
                    in_time_base,
                    out_time_base,
                )?;

                count += 1;
            }
        }

        if target.frames().map(|f| count >= f).unwrap_or(false) {
            break;
        }
    }

    transcoder.flush(
        &mut decoded,
        &mut encoded,
        &mut octx,
        in_time_base,
        out_time_base,
    )?;

    octx.write_trailer()?;

    Ok(())
}
