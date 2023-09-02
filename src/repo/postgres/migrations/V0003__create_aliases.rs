use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("aliases", |t| {
        t.add_column(
            "alias",
            types::text()
                .size(60)
                .primary(true)
                .unique(true)
                .nullable(false),
        );
        t.add_column("hash", types::binary().nullable(false));
        t.add_column("token", types::text().size(60).nullable(false));

        t.add_foreign_key(&["hash"], "hashes", &["hash"]);
        t.add_index("aliases_hash_index", types::index(["hash"]));
    });

    m.make::<Pg>().to_string()
}
