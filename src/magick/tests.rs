use super::{Details, DetailsOutput};

fn details_tests() -> [(&'static str, Details); 8] {
    [
        (
            "avif",
            Details {
                mime_type: super::image_avif(),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "gif",
            Details {
                mime_type: mime::IMAGE_GIF,
                width: 414,
                height: 261,
                frames: Some(17),
            },
        ),
        (
            "jpeg",
            Details {
                mime_type: mime::IMAGE_JPEG,
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "jxl",
            Details {
                mime_type: super::image_jxl(),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
        (
            "mp4",
            Details {
                mime_type: super::video_mp4(),
                width: 414,
                height: 261,
                frames: Some(17),
            },
        ),
        (
            "png",
            Details {
                mime_type: mime::IMAGE_PNG,
                width: 497,
                height: 694,
                frames: None,
            },
        ),
        (
            "webm",
            Details {
                mime_type: super::video_webm(),
                width: 112,
                height: 112,
                frames: Some(27),
            },
        ),
        (
            "webp",
            Details {
                mime_type: super::image_webp(),
                width: 1920,
                height: 1080,
                frames: None,
            },
        ),
    ]
}

#[test]
fn parse_details() {
    for (case, expected) in details_tests() {
        let string =
            std::fs::read_to_string(format!("./src/magick/magick_7_1_1_{case}_details.json"))
                .expect("Read file");

        let json: Vec<DetailsOutput> = serde_json::from_str(&string).expect("Valid json");

        let output = super::parse_details(json).expect("Parsed details");

        assert_eq!(output, expected);
    }
}
