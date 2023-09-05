#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, diesel_derive_enum::DbEnum)]
#[ExistingTypePath = "crate::repo::postgres::schema::sql_types::JobStatus"]
pub(super) enum JobStatus {
    New,
    Running,
}
