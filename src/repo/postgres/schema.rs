// @generated automatically by Diesel CLI.

pub mod sql_types {
    #[derive(diesel::query_builder::QueryId, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "job_status"))]
    pub struct JobStatus;
}

diesel::table! {
    aliases (alias) {
        alias -> Text,
        hash -> Text,
        token -> Text,
    }
}

diesel::table! {
    details (identifier) {
        identifier -> Text,
        json -> Jsonb,
    }
}

diesel::table! {
    hashes (hash) {
        hash -> Text,
        identifier -> Text,
        motion_identifier -> Nullable<Text>,
        created_at -> Timestamp,
        blurhash -> Nullable<Text>,
    }
}

diesel::table! {
    use diesel::sql_types::*;
    use super::sql_types::JobStatus;

    job_queue (id) {
        id -> Uuid,
        queue -> Text,
        job -> Jsonb,
        worker -> Nullable<Uuid>,
        status -> JobStatus,
        queue_time -> Timestamp,
        heartbeat -> Nullable<Timestamp>,
        unique_key -> Nullable<Text>,
        retry -> Int4,
    }
}

diesel::table! {
    proxies (url) {
        url -> Text,
        alias -> Text,
        accessed -> Timestamp,
    }
}

diesel::table! {
    refinery_schema_history (version) {
        version -> Int4,
        #[max_length = 255]
        name -> Nullable<Varchar>,
        #[max_length = 255]
        applied_on -> Nullable<Varchar>,
        #[max_length = 255]
        checksum -> Nullable<Varchar>,
    }
}

diesel::table! {
    settings (key) {
        key -> Text,
        value -> Text,
    }
}

diesel::table! {
    store_migrations (old_identifier) {
        old_identifier -> Text,
        new_identifier -> Text,
    }
}

diesel::table! {
    uploads (id) {
        id -> Uuid,
        result -> Nullable<Jsonb>,
        created_at -> Timestamp,
    }
}

diesel::table! {
    variants (id) {
        id -> Uuid,
        hash -> Text,
        variant -> Text,
        identifier -> Text,
        accessed -> Timestamp,
    }
}

diesel::joinable!(aliases -> hashes (hash));
diesel::joinable!(proxies -> aliases (alias));
diesel::joinable!(variants -> hashes (hash));

diesel::allow_tables_to_appear_in_same_query!(
    aliases,
    details,
    hashes,
    job_queue,
    proxies,
    refinery_schema_history,
    settings,
    store_migrations,
    uploads,
    variants,
);
