use std::path::PathBuf;

use uuid::Uuid;

pub(crate) fn generate_disk(mut path: PathBuf) -> PathBuf {
    path.extend(generate());
    path
}

pub(crate) fn generate_object() -> String {
    generate().join("/")
}

fn generate() -> Vec<String> {
    Uuid::now_v7()
        .into_bytes()
        .into_iter()
        .map(to_hex)
        .collect()
}

fn to_hex(byte: u8) -> String {
    format!("{byte:x}")
}
