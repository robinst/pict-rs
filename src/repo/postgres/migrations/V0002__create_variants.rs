use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("variants", |t| {
        t.inject_custom(r#""id" UUID PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL UNIQUE"#);
        t.add_column("hash", types::text().nullable(false));
        t.add_column("variant", types::text().nullable(false));
        t.add_column("identifier", types::text().nullable(false));
        t.add_column(
            "accessed",
            types::datetime()
                .nullable(false)
                .default(AutogenFunction::CurrentTimestamp),
        );

        t.add_foreign_key(&["hash"], "hashes", &["hash"]);
        t.add_index(
            "hash_variant_index",
            types::index(["hash", "variant"]).unique(true),
        );
    });

    m.make::<Pg>().to_string()
}
