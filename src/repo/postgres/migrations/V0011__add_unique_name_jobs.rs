use barrel::backend::Pg;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.change_table("job_queue", |t| {
        t.add_column(
            "unique_key",
            types::text().size(50).nullable(true).unique(true),
        );
    });

    m.make::<Pg>().to_string()
}
