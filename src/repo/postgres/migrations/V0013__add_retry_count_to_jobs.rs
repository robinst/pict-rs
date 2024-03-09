use barrel::backend::Pg;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.change_table("job_queue", |t| {
        t.add_column("retry", types::integer().nullable(false).default(5));
    });

    m.make::<Pg>().to_string()
}
