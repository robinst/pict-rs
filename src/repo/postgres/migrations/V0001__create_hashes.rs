use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("hashes", |t| {
        t.add_column(
            "hash",
            types::text()
                .primary(true)
                .unique(true)
                .nullable(false)
                .size(128),
        );
        t.add_column("identifier", types::text().unique(true).nullable(false));
        t.add_column(
            "motion_identifier",
            types::text().unique(true).nullable(true),
        );
        t.add_column(
            "created_at",
            types::datetime()
                .nullable(false)
                .default(AutogenFunction::CurrentTimestamp),
        );

        t.add_index("ordered_hash_index", types::index(["created_at", "hash"]));
    });

    m.make::<Pg>().to_string()
}
