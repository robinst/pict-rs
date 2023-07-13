use super::{Details, DetailsOutput, PixelFormatOutput};

fn details_tests() -> [(&'static str, Option<Details>); 10] {
    [
        (
            "apng",
            Some(Details {
                mime_type: crate::formats::mimes::image_apng(),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        ("avif", None),
        (
            "gif",
            Some(Details {
                mime_type: mime::IMAGE_GIF,
                width: 160,
                height: 227,
                frames: Some(28),
            }),
        ),
        ("jpeg", None),
        ("jxl", None),
        (
            "mp4",
            Some(Details {
                mime_type: crate::formats::mimes::video_mp4(),
                width: 852,
                height: 480,
                frames: Some(35364),
            }),
        ),
        ("png", None),
        (
            "webm",
            Some(Details {
                mime_type: crate::formats::mimes::video_webm(),
                width: 640,
                height: 480,
                frames: Some(34650),
            }),
        ),
        (
            "webm_av1",
            Some(Details {
                mime_type: crate::formats::mimes::video_webm(),
                width: 112,
                height: 112,
                frames: Some(27),
            }),
        ),
        ("webp", None),
    ]
}

#[test]
fn parse_details() {
    for (case, expected) in details_tests() {
        let string =
            std::fs::read_to_string(format!("./src/ffmpeg/ffprobe_6_0_{case}_details.json"))
                .expect("Read file");

        let json: DetailsOutput = serde_json::from_str(&string).expect("Valid json");

        let output = super::parse_details(json).expect("Parsed details");

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
    let formats =
        std::fs::read_to_string("./src/ffmpeg/ffprobe_6_0_pixel_formats.json").expect("Read file");

    let json: PixelFormatOutput = serde_json::from_str(&formats).expect("Valid json");

    let output = super::parse_pixel_formats(json);

    for format in ALPHA_PIXEL_FORMATS {
        assert!(output.contains(*format), "Doesn't contain {format}");
    }
}
