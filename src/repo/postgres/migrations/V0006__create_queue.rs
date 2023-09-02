use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.inject_custom("CREATE TYPE job_status AS ENUM ('new', 'running');");

    m.create_table("queue", |t| {
        t.inject_custom(r#""id" UUID PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL UNIQUE"#);
        t.add_column("queue", types::text().size(50).nullable(false));
        t.add_column("job", types::custom("jsonb").nullable(false));
        t.add_column("status", types::custom("job_status").nullable(false));
        t.add_column(
            "queue_time",
            types::datetime()
                .nullable(false)
                .default(AutogenFunction::CurrentTimestamp),
        );
        t.add_column("heartbeat", types::datetime());

        t.add_index("queue_status_index", types::index(["queue", "status"]));
        t.add_index("heartbeat_index", types::index(["heartbeat"]));
    });

    m.inject_custom(
        r#"
CREATE OR REPLACE FUNCTION queue_status_notify()
	RETURNS trigger AS
$$
BEGIN
	PERFORM pg_notify('queue_status_channel', NEW.id::text);
	RETURN NEW;
END;
$$ LANGUAGE plpgsql;
    "#
        .trim(),
    );

    m.inject_custom(
        r#"
CREATE TRIGGER queue_status
	AFTER INSERT OR UPDATE OF status
	ON queue
	FOR EACH ROW
EXECUTE PROCEDURE queue_status_notify();
    "#
        .trim(),
    );

    m.make::<Pg>().to_string()
}
