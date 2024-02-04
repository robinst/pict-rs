pub(super) fn init_metrics() {
    describe_postgres();
    describe_middleware();
    describe_generate();
    describe_object_storage();
}

fn describe_postgres() {
    metrics::describe_counter!(
        POSTGRES_POOL_CONNECTION_CREATE,
        "How many connections to postgres have been made"
    );
    metrics::describe_counter!(
        POSTGRES_POOL_CONNECTION_RECYCLE,
        "How many connections to postgres have been recycled"
    );
    metrics::describe_counter!(
        POSTGRES_POOL_GET,
        "How many times a connection has been retrieved from the connection pool",
    );
    metrics::describe_histogram!(
        POSTGRES_POOL_GET_DURATION,
        "How long pict-rs spent waiting for postgres connections from the connection pool"
    );
    metrics::describe_counter!(
        POSTGRES_JOB_NOTIFIER_NOTIFIED,
        "How many background job notifications pict-rs has successfully processed from postgres"
    );
    metrics::describe_counter!(
        POSTGRES_UPLOAD_NOTIFIER_NOTIFIED,
        "How many upload completion notifications pict-rs has successfully processed from postgres"
    );
    metrics::describe_counter!(
        POSTGRES_NOTIFICATION,
        "How many notifications pict-rs has received from postgres",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_COUNT,
        "Timings for counting the total number of hashes pict-rs is storing"
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_BOUND,
        "Timings for retrieving a timestamp for a given hash",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_ORDERED_HASH,
        "Timings for retrieving the most recent hash and timestamp before a provided time",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_NEXT_HASHES,
        "Timings for retrieving the next page of hashes given an ordered-hash bound",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_PREV_HASH,
        "Timings for retrieving the hash to act as the next hash page's bound",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_FIRST_HASHES,
        "Timings for retrieving the first page of hashes",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_CREATE_HASH,
        "Timings for inserting a new hash",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_UPDATE_IDENTIFIER,
        "Timings for updating the identifier for a provided hash",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_IDENTIFIER,
        "Timings for fetching the identifier for a provided hash",
    );
    metrics::describe_histogram!(
        POSTGRES_VARIANTS_RELATE_VARIANT_IDENTIFIER,
        "Timings for inserting a variant and identifier for a provided hash",
    );
    metrics::describe_histogram!(
        POSTGRES_VARIANTS_IDENTIFIER,
        "Timings for fetching an identifier for a provided hash and variant"
    );
    metrics::describe_histogram!(
        POSTGRES_VARIANTS_FOR_HASH,
        "Timings for fetching all variants and identifiers for a provided hash"
    );
    metrics::describe_histogram!(
        POSTGRES_VARIANTS_REMOVE,
        "Timings for removing a variant for a provided hash",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_RELATE_MOTION_IDENTIFIER,
        "Timings for relating a still image identifier for a provided hash representing a video",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_MOTION_IDENTIFIER,
        "Timings for fetching a still image identifier for a provided hash representing a video",
    );
    metrics::describe_histogram!(
        POSTGRES_VARIANTS_CLEANUP,
        "Timings for deleting all variants for a provided hash",
    );
    metrics::describe_histogram!(
        POSTGRES_HASHES_CLEANUP,
        "Timings for deleting a provided hash",
    );
    metrics::describe_histogram!(
        POSTGRES_ALIASES_CREATE,
        "Timings for creating an alias for a provided hash",
    );
    metrics::describe_histogram!(
        POSTGRES_ALIASES_DELETE_TOKEN,
        "Timings for fetching a delete token for a provided alias",
    );
    metrics::describe_histogram!(
        POSTGRES_ALIASES_HASH,
        "Timings for fetching a hash for a provided alias",
    );
    metrics::describe_histogram!(
        POSTGRES_ALIASES_FOR_HASH,
        "Timings for fetching all aliases for a provided hash",
    );
    metrics::describe_histogram!(
        POSTGRES_ALIASES_CLEANUP,
        "Timings for deleting a provided alias",
    );
    metrics::describe_histogram!(POSTGRES_SETTINGS_SET, "Timings for setting a given setting");
    metrics::describe_histogram!(
        POSTGRES_SETTINGS_GET,
        "Timings for getting a provided setting"
    );
    metrics::describe_histogram!(
        POSTGRES_SETTINGS_REMOVE,
        "Timings for removing a provided setting"
    );
    metrics::describe_histogram!(
        POSTGRES_DETAILS_RELATE,
        "Timings for relating details to a provided identifier"
    );
    metrics::describe_histogram!(
        POSTGRES_DETAILS_GET,
        "Timings for getting details for a provided identifier",
    );
    metrics::describe_histogram!(
        POSTGRES_DETAILS_CLEANUP,
        "Timings for deleting details for a provided identifier",
    );
    metrics::describe_histogram!(
        POSTGRES_QUEUE_COUNT,
        "Timings for counting the size of the job queue",
    );
    metrics::describe_histogram!(
        POSTGRES_QUEUE_PUSH,
        "Timings for inserting a new job into the job queue",
    );
    metrics::describe_histogram!(
        POSTGRES_QUEUE_LISTEN,
        "Timings for initializing the queue listener",
    );
    metrics::describe_histogram!(
        POSTGRES_QUEUE_REQUEUE,
        "Timings for marking stale jobs as ready to pop"
    );
    metrics::describe_histogram!(
        POSTGRES_QUEUE_CLAIM,
        "Timings for claiming a job from the job queue",
    );
    metrics::describe_histogram!(
        POSTGRES_QUEUE_HEARTBEAT,
        "Timings for updating the provided job's keepalive heartbeat"
    );
    metrics::describe_histogram!(
        POSTGRES_QUEUE_COMPLETE,
        "Timings for removing a completed job from the queue",
    );
    metrics::describe_histogram!(
        POSTGRES_STORE_MIGRATION_COUNT,
        "Timings for fetching the count of files successfully migrated between stores",
    );
    metrics::describe_histogram!(
        POSTGRES_STORE_MIGRATION_MARK_MIGRATED,
        "Timings for marking a given identifier as having been migrated between stores",
    );
    metrics::describe_histogram!(
        POSTGRES_STORE_MIGRATION_IS_MIGRATED,
        "Timings for checking if a given identifier has been migrated between stores",
    );
    metrics::describe_histogram!(
        POSTGRES_STORE_MIGRATION_CLEAR,
        "Timings for clearing all records of identifiers migrated between stores. This occurs on successful migration"
    );
    metrics::describe_histogram!(
        POSTGRES_PROXY_RELATE_URL,
        "Timings for relating a provided proxy URL to an alias",
    );
    metrics::describe_histogram!(
        POSTGRES_PROXY_RELATED,
        "Timings for fetching a related alias for a provided proxy URL",
    );
    metrics::describe_histogram!(
        POSTGRES_PROXY_REMOVE_RELATION,
        "Timings for removing a proxy URL for a provied alias",
    );
    metrics::describe_histogram!(
        POSTGRES_ALIAS_ACCESS_SET_ACCESSED,
        "Timings for marking a given alias as having been accessed",
    );
    metrics::describe_histogram!(
        POSTGRES_ALIAS_ACCESS_ACCESSED_AT,
        "Timings for checking when a given alias was last accessed",
    );
    metrics::describe_histogram!(
        POSTGRES_ALIAS_ACCESS_OLDER_ALIASES,
        "Timings for fetching a page of aliases last accessed earlier than a given timestamp",
    );
    metrics::describe_histogram!(
        POSTGRES_VARIANT_ACCESS_SET_ACCESSED,
        "Timings for marking a given variant as having been accessed",
    );
    metrics::describe_histogram!(
        POSTGRES_VARIANT_ACCESS_ACCESSED_AT,
        "Timings for checking when a given variant was last accessed",
    );
    metrics::describe_histogram!(
        POSTGRES_VARIANT_ACCESS_OLDER_VARIANTS,
        "Timings for fetching a page of variants last accessed earlier than a given timestamp",
    );
    metrics::describe_histogram!(
        POSTGRES_UPLOADS_CREATE,
        "Timings for inserting a new upload ID",
    );
    metrics::describe_histogram!(
        POSTGRES_UPLOADS_LISTEN,
        "Timings for initializing the upload listener",
    );
    metrics::describe_histogram!(
        POSTGRES_UPLOADS_WAIT,
        "Timings for checking if a given upload is completed",
    );
    metrics::describe_histogram!(
        POSTGRES_UPLOADS_CLAIM,
        "Timings for claiming a given completed upload",
    );
    metrics::describe_histogram!(
        POSTGRES_UPLOADS_COMPLETE,
        "Timings for marking a given upload as completed",
    );
}

pub(crate) const POSTGRES_POOL_CONNECTION_CREATE: &str = "pict-rs.postgres.pool.connection.create";
pub(crate) const POSTGRES_POOL_CONNECTION_RECYCLE: &str =
    "pict-rs.postgres.pool.connection.recycle";
pub(crate) const POSTGRES_POOL_GET: &str = "pict-rs.postgres.pool.get";
pub(crate) const POSTGRES_POOL_GET_DURATION: &str = "pict-rs.postgres.pool.duration";
pub(crate) const POSTGRES_JOB_NOTIFIER_NOTIFIED: &str = "pict-rs.postgres.job-notifier.notified";
pub(crate) const POSTGRES_UPLOAD_NOTIFIER_NOTIFIED: &str =
    "pict-rs.postgres.upload-notifier.notified";
pub(crate) const POSTGRES_NOTIFICATION: &str = "pict-rs.postgres.notification";
pub(crate) const POSTGRES_HASHES_COUNT: &str = "pict-rs.postgres.hashes.count";
pub(crate) const POSTGRES_HASHES_BOUND: &str = "pict-rs.postgres.hashes.bound";
pub(crate) const POSTGRES_HASHES_ORDERED_HASH: &str = "pict-rs.postgres.hashes.ordered-hash";
pub(crate) const POSTGRES_HASHES_NEXT_HASHES: &str = "pict-rs.postgres.hashes.next-hashes";
pub(crate) const POSTGRES_HASHES_PREV_HASH: &str = "pict-rs.postgres.hashes.prev-hash";
pub(crate) const POSTGRES_HASHES_FIRST_HASHES: &str = "pict-rs.postgres.hashes.first-hashes";
pub(crate) const POSTGRES_HASHES_CREATE_HASH: &str = "pict-rs.postgres.hashes.create-hash";
pub(crate) const POSTGRES_HASHES_UPDATE_IDENTIFIER: &str =
    "pict-rs.postgres.hashes.update-identifier";
pub(crate) const POSTGRES_HASHES_IDENTIFIER: &str = "pict-rs.postgres.identifier";
pub(crate) const POSTGRES_VARIANTS_RELATE_VARIANT_IDENTIFIER: &str =
    "pict-rs.postgres.variants.relate-variant-identifier";
pub(crate) const POSTGRES_VARIANTS_IDENTIFIER: &str = "pict-rs.postgres.variants.identifier";
pub(crate) const POSTGRES_VARIANTS_FOR_HASH: &str = "pict-rs.postgres.variants.for-hash";
pub(crate) const POSTGRES_VARIANST_REMOVE: &str = "pict-rs.postgres.variants.remove";
pub(crate) const POSTGRES_HASHES_RELATE_MOTION_IDENTIFIER: &str =
    "pict-rs.postgres.hashes.relate-motion-identifier";
pub(crate) const POSTGRES_HASHES_MOTION_IDENTIFIER: &str =
    "pict-rs.postgres.hashes.motion-identifier";
pub(crate) const POSTGRES_VARIANTS_CLEANUP: &str = "pict-rs.postgres.variants.cleanup";
pub(crate) const POSTGRES_HASHES_CLEANUP: &str = "pict-rs.postgres.hashes.cleanup";
pub(crate) const POSTGRES_ALIASES_CREATE: &str = "pict-rs.postgres.aliases.create";
pub(crate) const POSTGRES_ALIASES_DELETE_TOKEN: &str = "pict-rs.postgres.aliases.delete-token";
pub(crate) const POSTGRES_ALIASES_HASH: &str = "pict-rs.postgres.aliases.hash";
pub(crate) const POSTGRES_ALIASES_FOR_HASH: &str = "pict-rs.postgres.aliases.for-hash";
pub(crate) const POSTGRES_ALIASES_CLEANUP: &str = "pict-rs.postgres.aliases.cleanup";
pub(crate) const POSTGRES_SETTINGS_SET: &str = "pict-rs.postgres.settings.set";
pub(crate) const POSTGRES_SETTINGS_GET: &str = "pict-rs.postgres.settings.get";
pub(crate) const POSTGRES_SETTINGS_REMOVE: &str = "pict-rs.postgres.settings.remove";
pub(crate) const POSTGRES_DETAILS_RELATE: &str = "pict-rs.postgres.details.relate";
pub(crate) const POSTGRES_DETAILS_GET: &str = "pict-rs.postgres.details.get";
pub(crate) const POSTGRES_DETAILS_CLEANUP: &str = "pict-rs.postgres.details.cleanup";
pub(crate) const POSTGRES_QUEUE_COUNT: &str = "pict-rs.postgres.queue.count";
pub(crate) const POSTGRES_QUEUE_PUSH: &str = "pict-rs.postgres.queue.push";
pub(crate) const POSTGRES_QUEUE_LISTEN: &str = "pict-rs.postgres.queue.listen";
pub(crate) const POSTGRES_QUEUE_REQUEUE: &str = "pict-rs.postgres.queue.requeue";
pub(crate) const POSTGRES_QUEUE_CLAIM: &str = "pict-rs.postgres.queue.claim";
pub(crate) const POSTGRES_QUEUE_HEARTBEAT: &str = "pict-rs.postgres.queue.heartbeat";
pub(crate) const POSTGRES_QUEUE_COMPLETE: &str = "pict-rs.postgres.queue.complete";
pub(crate) const POSTGRES_STORE_MIGRATION_COUNT: &str = "pict-rs.postgres.store-migration.count";
pub(crate) const POSTGRES_STORE_MIGRATION_MARK_MIGRATED: &str =
    "pict-rs.postgres.store-migration.mark-migrated";
pub(crate) const POSTGRES_STORE_MIGRATION_IS_MIGRATED: &str =
    "pict-rs.postgres.store-migration.is-migrated";
pub(crate) const POSTGRES_STORE_MIGRATION_CLEAR: &str = "pict-rs.postgres.store-migration.clear";
pub(crate) const POSTGRES_PROXY_RELATE_URL: &str = "pict-rs.postgres.proxy.relate-url";
pub(crate) const POSTGRES_PROXY_RELATED: &str = "pict-rs.postgres.proxy.related";
pub(crate) const POSTGRES_PROXY_REMOVE_RELATION: &str = "pict-rs.postgres.proxy.remove-relation";
pub(crate) const POSTGRES_ALIAS_ACCESS_SET_ACCESSED: &str =
    "pict-rs.postgres.alias-access.set-accessed";
pub(crate) const POSTGRES_ALIAS_ACCESS_ACCESSED_AT: &str =
    "pict-rs.postgres.alias-access.accessed-at";
pub(crate) const POSTGRES_ALIAS_ACCESS_OLDER_ALIASES: &str =
    "pict-rs.postgres.alias-access.older-aliases";
pub(crate) const POSTGRES_VARIANT_ACCESS_SET_ACCESSED: &str =
    "pict-rs.postgres.variant-access.set-accessed";
pub(crate) const POSTGRES_VARIANT_ACCESS_ACCESSED_AT: &str =
    "pict-rs.postgres.variant-access.accessed-at";
pub(crate) const POSTGRES_VARIANT_ACCESS_OLDER_VARIANTS: &str =
    "pict-rs.postgres.variant-access.older-variants";
pub(crate) const POSTGRES_UPLOADS_CREATE: &str = "pict-rs.postgres.uploads.create";
pub(crate) const POSTGRES_UPLOADS_LISTEN: &str = "pict-rs.postgres.uploads.listen";
pub(crate) const POSTGRES_UPLOADS_WAIT: &str = "pict-rs.postgres.uploads.wait";
pub(crate) const POSTGRES_UPLOADS_CLAIM: &str = "pict-rs.postgres.uploads.claim";
pub(crate) const POSTGRES_UPLOADS_COMPLETE: &str = "pict-rs.postgres.uploads.complete";

fn describe_middleware() {
    metrics::describe_counter!(
        REQUEST_START,
        "How many requests have been made to pict-rs, by requested path"
    );
    metrics::describe_counter!(
        REQUEST_END,
        "How many requests pict-rs has finished serving, by requested path"
    );
    metrics::describe_histogram!(
        REQUEST_TIMINGS,
        "How long pict-rs takes to serve requests, by requested path"
    );
}

pub(crate) const REQUEST_START: &str = "pict-rs.request.start";
pub(crate) const REQUEST_END: &str = "pict-rs.request.end";
pub(crate) const REQUEST_TIMINGS: &str = "pict-rs.request.timings";

fn describe_generate() {
    metrics::describe_counter!(
        GENERATE_START,
        "Counter describing how many times a variant has begun processing"
    );
    metrics::describe_histogram!(
        GENERATE_DURATION,
        "Timings for processing variants (i.e. generating thumbnails)"
    );
    metrics::describe_counter!(GENERATE_END, "Counter describing how many times a variant has finished processing, and whether it completed or aborted");
    metrics::describe_histogram!(
        GENERATE_PROCESS,
        "Timings for processing media or waiting for media to be processed"
    );
}

pub(crate) const GENERATE_START: &str = "pict-rs.generate.start";
pub(crate) const GENERATE_DURATION: &str = "pict-rs.generate.duration";
pub(crate) const GENERATE_END: &str = "pict-rs.generate.end";
pub(crate) const GENERATE_PROCESS: &str = "pict-rs.generate.process";

fn describe_object_storage() {
    metrics::describe_historgram!(
        OBJECT_STORAGE_HEAD_BUCKET_REQUEST,
        "Timings for HEAD requests for the pict-rs Bucket in object storage"
    );
    metrics::describe_historgram!(
        OBJECT_STORAGE_PUT_OBJECT_REQUEST,
        "Timings for PUT requests for uploading media to object storage"
    );
    metrics::describe_historgram!(OBJECT_STORAGE_CREATE_MULTIPART_REQUEST, "Timings for creating a multipart request to object storage. This is the first step in uploading larger files.");
    metrics::describe_historgram!(OBJECT_STORAGE_CREATE_UPLOAD_PART_REQUEST, "Timings for uploading part of a large file to object storage as a multipart part. This is one step in uploading larger files.");
    metrics::describe_historgram!(
        OBJECT_STORAGE_ABORT_MULTIPART_REQUEST,
        "Timings for aborting a multipart upload to object storage"
    );
    metrics::describe_historgram!(
        OBJECT_STORAGE_GET_OBJECT_REQUEST,
        "Timings for requesting media from object storage"
    );
    metrics::describe_historgram!(
        OBJECT_STORAGE_GET_OBJECT_REQUEST_STREAM,
        "Timings for streaming an object from object storage"
    );
    metrics::describe_historgram!(
        OBJECT_STORAGE_HEAD_OBJECT_REQUEST,
        "Timings for requesting metadata for media from object storage"
    );
    metrics::describe_historgram!(
        OBJECT_STORAGE_DELETE_OBJECT_REQUEST,
        "Timings for requesting media in object storage be deleted"
    );
    metrics::describe_historgram!(
        OBJECT_STORAGE_COMPLETE_MULTIPART_REQUEST,
        "Timings for completing a multipart request to object storage"
    );
}

pub(crate) const OBJECT_STORAGE_HEAD_BUCKET_REQUEST: &str =
    "pict-rs.object-storage.head-bucket-request";
pub(crate) const OBJECT_STORAGE_PUT_OBJECT_REQUEST: &str =
    "pict-rs.object-storage.put-object-request";
pub(crate) const OBJECT_STORAGE_CREATE_MULTIPART_REQUEST: &str =
    "pict-rs.object-storage.create-multipart-request";
pub(crate) const OBJECT_STORAGE_CREATE_UPLOAD_PART_REQUEST: &str =
    "pict-rs.object-storage.create-upload-part-request";
pub(crate) const OBJECT_STORAGE_ABORT_MULTIPART_REQUEST: &str =
    "pict-rs.object-storage.abort-multipart-request";
pub(crate) const OBJECT_STORAGE_GET_OBJECT_REQUEST: &str =
    "pict-rs.object-storage.get-object-request";
pub(crate) const OBJECT_STORAGE_GET_OBJECT_REQUEST_STREAM: &str =
    "pict-rs.object-storage.get-object-request.stream";
pub(crate) const OBJECT_STORAGE_HEAD_OBJECT_REQUEST: &str =
    "pict-rs.object-storage.head-object-request";
pub(crate) const OBJECT_STORAGE_DELETE_OBJECT_REQUEST: &str =
    "pict-rs.object-storage.delete-object-request";
pub(crate) const OBJECT_STORAGE_COMPLETE_MULTIPART_REQUEST: &str =
    "pict-rs.object-storage.complete-multipart-request";
