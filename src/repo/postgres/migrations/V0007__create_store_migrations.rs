use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("store_migrations", |t| {
        t.add_column(
            "identifier",
            types::text().primary(true).nullable(false).unique(true),
        );
    });

    m.make::<Pg>().to_string()
}
