use crate::formats::{
    AlphaCodec, AnimationFormat, ImageFormat, ImageInput, InputFile, InputVideoFormat,
    Mp4AudioCodec, Mp4Codec, WebmAlphaCodec, WebmCodec,
};

use super::{Discovery, FfMpegDiscovery, PixelFormatOutput};

fn details_tests() -> [(&'static str, Option<Discovery>); 14] {
    [
        (
            "animated_webp",
            Some(Discovery {
                input: InputFile::Animation(AnimationFormat::Webp),
                width: 0,
                height: 0,
                frames: None,
            }),
        ),
        (
            "animated_avif",
            Some(Discovery {
                input: InputFile::Animation(AnimationFormat::Avif),
                width: 112,
                height: 112,
                frames: None,
            }),
        ),
        (
            "apng",
            Some(Discovery {
                input: InputFile::Animation(AnimationFormat::Apng),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        (
            "avif",
            Some(Discovery {
                input: InputFile::Animation(AnimationFormat::Avif),
                width: 1200,
                height: 1387,
                frames: None,
            }),
        ),
        (
            "gif",
            Some(Discovery {
                input: InputFile::Animation(AnimationFormat::Gif),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        (
            "jpeg",
            Some(Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Jpeg,
                    needs_reorient: false,
                }),
                width: 1663,
                height: 1247,
                frames: None,
            }),
        ),
        (
            "jxl",
            Some(Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Jxl,
                    needs_reorient: false,
                }),
                width: 0,
                height: 0,
                frames: None,
            }),
        ),
        (
            "mp4",
            Some(Discovery {
                input: InputFile::Video(InputVideoFormat::Mp4 {
                    video_codec: Mp4Codec::H264,
                    audio_codec: None,
                }),
                width: 1426,
                height: 834,
                frames: Some(105),
            }),
        ),
        (
            "mp4_av1",
            Some(Discovery {
                input: InputFile::Video(InputVideoFormat::Mp4 {
                    video_codec: Mp4Codec::Av1,
                    audio_codec: None,
                }),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        (
            "png",
            Some(Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Png,
                    needs_reorient: false,
                }),
                width: 450,
                height: 401,
                frames: None,
            }),
        ),
        (
            "webm",
            Some(Discovery {
                input: InputFile::Video(InputVideoFormat::Webm {
                    video_codec: WebmCodec::Alpha(AlphaCodec {
                        alpha: false,
                        codec: WebmAlphaCodec::Vp9,
                    }),
                    audio_codec: None,
                }),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        (
            "webm_av1",
            Some(Discovery {
                input: InputFile::Video(InputVideoFormat::Webm {
                    video_codec: WebmCodec::Av1,
                    audio_codec: None,
                }),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        (
            "webp",
            Some(Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Webp,
                    needs_reorient: false,
                }),
                width: 1200,
                height: 1387,
                frames: None,
            }),
        ),
        (
            "mov",
            Some(Discovery {
                input: InputFile::Video(InputVideoFormat::Mp4 {
                    video_codec: Mp4Codec::H265,
                    audio_codec: Some(Mp4AudioCodec::Aac),
                }),
                width: 1920,
                height: 1080,
                frames: Some(187),
            }),
        ),
    ]
}

#[test]
fn parse_discovery() {
    for (case, expected) in details_tests() {
        let string = std::fs::read_to_string(format!(
            "./src/discover/ffmpeg/ffprobe_6_0_{case}_details.json"
        ))
        .expect("Read file");

        let json: FfMpegDiscovery = serde_json::from_str(&string).expect("Valid json");

        let (output, _) = super::parse_discovery(json).expect("Parsed details");

        assert_eq!(output, expected);
    }
}

const ALPHA_PIXEL_FORMATS: &[&str] = &[
    "pal8",
    "argb",
    "rgba",
    "abgr",
    "bgra",
    "yuva420p",
    "ya8",
    "yuva422p",
    "yuva444p",
    "yuva420p9be",
    "yuva420p9le",
    "yuva422p9be",
    "yuva422p9le",
    "yuva444p9be",
    "yuva444p9le",
    "yuva420p10be",
    "yuva420p10le",
    "yuva422p10be",
    "yuva422p10le",
    "yuva444p10be",
    "yuva444p10le",
    "yuva420p16be",
    "yuva420p16le",
    "yuva422p16be",
    "yuva422p16le",
    "yuva444p16be",
    "yuva444p16le",
    "rgba64be",
    "rgba64le",
    "bgra64be",
    "bgra64le",
    "ya16be",
    "ya16le",
    "gbrap",
    "gbrap16be",
    "gbrap16le",
    "ayuv64le",
    "ayuv64be",
    "gbrap12be",
    "gbrap12le",
    "gbrap10be",
    "gbrap10le",
    "gbrapf32be",
    "gbrapf32le",
    "yuva422p12be",
    "yuva422p12le",
    "yuva444p12be",
    "yuva444p12le",
    "vuya",
    "rgbaf16be",
    "rgbaf16le",
    "rgbaf32be",
    "rgbaf32le",
];

#[test]
fn parse_pixel_formats() {
    let formats = std::fs::read_to_string("./src/discover/ffmpeg/ffprobe_6_0_pixel_formats.json")
        .expect("Read file");

    let json: PixelFormatOutput = serde_json::from_str(&formats).expect("Valid json");

    let output = super::parse_pixel_formats(json);

    for format in ALPHA_PIXEL_FORMATS {
        assert!(output.contains(*format), "Doesn't contain {format}");
    }
}
