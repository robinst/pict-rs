use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("details", |t| {
        t.add_column(
            "identifier",
            types::text().primary(true).unique(true).nullable(false),
        );
        t.add_column("details", types::custom("jsonb").nullable(false));
    });

    m.make::<Pg>().to_string()
}
