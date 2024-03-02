use std::path::PathBuf;

use uuid::Uuid;

fn add_extension(filename: String, extension: Option<&str>) -> String {
    if let Some(extension) = extension {
        filename + extension
    } else {
        filename
    }
}

pub(crate) fn generate_disk(mut path: PathBuf, extension: Option<&str>) -> PathBuf {
    let (directories, filename) = generate();
    path.extend(directories);
    path.push(add_extension(filename, extension));
    path
}

pub(crate) fn generate_object(extension: Option<&str>) -> String {
    let (directories, filename) = generate();

    format!(
        "{}/{}",
        directories.join("/"),
        add_extension(filename, extension)
    )
}

fn generate() -> (Vec<String>, String) {
    let s = Uuid::now_v7().simple().to_string();

    let directories = (0..10).map(|i| s[i * 2..i * 2 + 2].to_string()).collect();
    let filename = s[20..].to_string();

    (directories, filename)
}
