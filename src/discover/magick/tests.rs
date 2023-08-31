use crate::formats::{AnimationFormat, ImageFormat, ImageInput, InputFile};

use super::{Discovery, MagickDiscovery};

fn details_tests() -> [(&'static str, Discovery); 7] {
    [
        (
            "animated_webp",
            Discovery {
                input: InputFile::Animation(AnimationFormat::Webp),
                width: 112,
                height: 112,
                frames: Some(27),
            },
        ),
        (
            "avif",
            Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Avif,
                    needs_reorient: false,
                }),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "gif",
            Discovery {
                input: InputFile::Animation(AnimationFormat::Gif),
                width: 414,
                height: 261,
                frames: Some(17),
            },
        ),
        (
            "jpeg",
            Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Jpeg,
                    needs_reorient: false,
                }),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "jxl",
            Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Jxl,
                    needs_reorient: false,
                }),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "png",
            Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Png,
                    needs_reorient: false,
                }),
                width: 497,
                height: 694,
                frames: None,
            },
        ),
        (
            "webp",
            Discovery {
                input: InputFile::Image(ImageInput {
                    format: ImageFormat::Webp,
                    needs_reorient: false,
                }),
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

        let output = super::parse_discovery(json).expect("Parsed details");

        assert_eq!(output, expected);
    }
}
