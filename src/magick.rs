fn thumbnail_args(max_dimension: usize) -> [String; 2] {
    [
        "-sample".to_string(),
        format!("{}x{}>", max_dimension, max_dimension),
    ]
}

fn resize_args(max_dimension: usize) -> [String; 4] {
    [
        "-filter".to_string(),
        "Lanczos".to_string(),
        "-resize".to_string(),
        format!("{}x{}>", max_dimension, max_dimension),
    ]
}

fn crop_args(width: usize, height: usize) -> [String; 4] {
    [
        "-gravity".to_string(),
        "center".to_string(),
        "-crop".to_string(),
        format!("{}x{}>", width, height),
    ]
}

fn blur_args(radius: f64) -> [String; 2] {
    ["-gaussian-blur".to_string(), radius.to_string()]
}
