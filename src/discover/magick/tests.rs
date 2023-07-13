use crate::formats::{AnimationFormat, ImageFormat, InternalFormat, InternalVideoFormat};

use super::{DiscoveryLite, MagickDiscovery};

fn details_tests() -> [(&'static str, DiscoveryLite); 9] {
    [
        (
            "animated_webp",
            DiscoveryLite {
                format: InternalFormat::Animation(AnimationFormat::Webp),
                width: 112,
                height: 112,
                frames: Some(27),
            },
        ),
        (
            "avif",
            DiscoveryLite {
                format: InternalFormat::Image(ImageFormat::Avif),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "gif",
            DiscoveryLite {
                format: InternalFormat::Animation(AnimationFormat::Gif),
                width: 414,
                height: 261,
                frames: Some(17),
            },
        ),
        (
            "jpeg",
            DiscoveryLite {
                format: InternalFormat::Image(ImageFormat::Jpeg),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "jxl",
            DiscoveryLite {
                format: InternalFormat::Image(ImageFormat::Jxl),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "mp4",
            DiscoveryLite {
                format: InternalFormat::Video(InternalVideoFormat::Mp4),
                width: 414,
                height: 261,
                frames: Some(17),
            },
        ),
        (
            "png",
            DiscoveryLite {
                format: InternalFormat::Image(ImageFormat::Png),
                width: 497,
                height: 694,
                frames: None,
            },
        ),
        (
            "webm",
            DiscoveryLite {
                format: InternalFormat::Video(InternalVideoFormat::Webm),
                width: 112,
                height: 112,
                frames: Some(27),
            },
        ),
        (
            "webp",
            DiscoveryLite {
                format: InternalFormat::Image(ImageFormat::Webp),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
    ]
}

#[test]
fn parse_discovery() {
    for (case, expected) in details_tests() {
        let string = std::fs::read_to_string(format!(
            "./src/discover/magick/magick_7_1_1_{case}_details.json"
        ))
        .expect("Read file");

        let json: Vec<MagickDiscovery> = serde_json::from_str(&string).expect("Valid json");

        let output = super::parse_discovery(json).expect("Parsed details").lite();

        assert_eq!(output, expected);
    }
}
