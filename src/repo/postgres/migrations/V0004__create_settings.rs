use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("settings", |t| {
        t.add_column(
            "key",
            types::text()
                .size(80)
                .primary(true)
                .unique(true)
                .nullable(false),
        );
        t.add_column("value", types::text().size(80).nullable(false));
    });

    m.make::<Pg>().to_string()
}
