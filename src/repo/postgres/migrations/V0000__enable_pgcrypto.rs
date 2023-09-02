use barrel::backend::Pg;
use barrel::functions::AutogenFunction;
use barrel::{types, Migration};

pub(crate) fn migration() -> String {
    let mut m = Migration::new();

    m.inject_custom("CREATE EXTENSION pgcrypto;");

    m.make::<Pg>().to_string()
}
