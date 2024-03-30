use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("keyed_notifications", |t| {
        t.add_column(
            "key",
            types::text().primary(true).unique(true).nullable(false),
        );
        t.add_column(
            "heartbeat",
            types::datetime()
                .nullable(false)
                .default(AutogenFunction::CurrentTimestamp),
        );

        t.add_index(
            "keyed_notifications_heartbeat_index",
            types::index(["heartbeat"]),
        );
    });

    m.inject_custom(
        r#"
CREATE OR REPLACE FUNCTION keyed_notify()
	RETURNS trigger AS
$$
BEGIN
	PERFORM pg_notify('keyed_notification_channel', OLD.key);
	RETURN NEW;
END;
$$ LANGUAGE plpgsql;
    "#
        .trim(),
    );

    m.inject_custom(
        r#"
CREATE TRIGGER keyed_notification_removed
	AFTER DELETE
	ON keyed_notifications
	FOR EACH ROW
EXECUTE PROCEDURE keyed_notify();
    "#,
    );
    m.make::<Pg>().to_string()
}
