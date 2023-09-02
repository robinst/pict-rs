use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.create_table("uploads", |t| {
        t.inject_custom(r#""id" UUID PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL UNIQUE"#);
        t.add_column("result", types::custom("jsonb"));
    });

    m.inject_custom(
        r#"
CREATE OR REPLACE FUNCTION upload_completion_notify()
	RETURNS trigger AS
$$
BEGIN
	PERFORM pg_notify('upload_completion_channel', NEW.id::text);
	RETURN NEW;
END;
$$ LANGUAGE plpgsql;
    "#
        .trim(),
    );

    m.inject_custom(
        r#"
CREATE TRIGGER upload_result
	AFTER INSERT OR UPDATE OF result
	ON uploads
	FOR EACH ROW
EXECUTE PROCEDURE upload_completion_notify();
    "#,
    );

    m.make::<Pg>().to_string()
}
