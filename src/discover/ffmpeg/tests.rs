use crate::formats::{AnimationFormat, ImageFormat, InternalFormat, InternalVideoFormat};

use super::{DiscoveryLite, FfMpegDiscovery, PixelFormatOutput};

fn details_tests() -> [(&'static str, Option<DiscoveryLite>); 11] {
    [
        (
            "animated_webp",
            Some(DiscoveryLite {
                format: InternalFormat::Animation(AnimationFormat::Webp),
                width: 0,
                height: 0,
                frames: None,
            }),
        ),
        (
            "apng",
            Some(DiscoveryLite {
                format: InternalFormat::Animation(AnimationFormat::Apng),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        (
            "avif",
            Some(DiscoveryLite {
                format: InternalFormat::Animation(AnimationFormat::Avif),
                width: 1920,
                height: 1080,
                frames: None,
            }),
        ),
        (
            "gif",
            Some(DiscoveryLite {
                format: InternalFormat::Animation(AnimationFormat::Gif),
                width: 160,
                height: 227,
                frames: Some(28),
            }),
        ),
        ("jpeg", None),
        ("jxl", None),
        (
            "mp4",
            Some(DiscoveryLite {
                format: InternalFormat::Video(InternalVideoFormat::Mp4),
                width: 852,
                height: 480,
                frames: Some(35364),
            }),
        ),
        (
            "png",
            Some(DiscoveryLite {
                format: InternalFormat::Image(ImageFormat::Png),
                width: 450,
                height: 401,
                frames: None,
            }),
        ),
        (
            "webm",
            Some(DiscoveryLite {
                format: InternalFormat::Video(InternalVideoFormat::Webm),
                width: 640,
                height: 480,
                frames: Some(34650),
            }),
        ),
        (
            "webm_av1",
            Some(DiscoveryLite {
                format: InternalFormat::Video(InternalVideoFormat::Webm),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        (
            "webp",
            Some(DiscoveryLite {
                format: InternalFormat::Image(ImageFormat::Webp),
                width: 1920,
                height: 1080,
                frames: None,
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

        let output = super::parse_discovery(json).expect("Parsed details");

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
