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
    let s = Uuid::now_v7().simple().to_string();

    (0..10).map(|i| s[i * 2..i * 2 + 2].to_string()).collect()
}
