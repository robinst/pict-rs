use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("proxies", |t| {
        t.add_column(
            "url",
            types::text().primary(true).unique(true).nullable(false),
        );
        t.add_column("alias", types::text().nullable(false));
        t.add_column(
            "accessed",
            types::datetime()
                .nullable(false)
                .default(AutogenFunction::CurrentTimestamp),
        );

        t.add_foreign_key(&["alias"], "aliases", &["alias"]);
    });

    m.make::<Pg>().to_string()
}
