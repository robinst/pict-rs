use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("store_migrations", |t| {
        t.add_column(
            "old_identifier",
            types::text().primary(true).nullable(false).unique(true),
        );
        t.add_column("new_identifier", types::text().nullable(false).unique(true));
    });

    m.make::<Pg>().to_string()
}
