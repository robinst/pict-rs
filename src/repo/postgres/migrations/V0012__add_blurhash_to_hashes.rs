use barrel::backend::Pg;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.change_table("hashes", |t| {
        t.add_column(
            "blurhash",
            types::text().size(60).nullable(true).unique(false),
        );
    });

    m.make::<Pg>().to_string()
}
