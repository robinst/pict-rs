use barrel::backend::Pg;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.inject_custom("CREATE INDEX queue_time_index ON job_queue (queue_time);");
    m.inject_custom("DROP INDEX queue_status_index;");

    m.make::<Pg>().to_string()
}
