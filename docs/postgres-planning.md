# Planning for implementing a postgres repo for pict-rs

## Technology

I've identified these crates as useful for achieving a reasonable postgres experience
- [diesel-async](https://docs.rs/diesel-async/latest/diesel_async/)
- [refinery](https://docs.rs/refinery/latest/refinery/)
- [deadpool](https://docs.rs/deadpool/latest/deadpool/)
- [tokio-postgres](https://docs.rs/tokio-postgres/latest/tokio_postgres/)

tokio-postgres will actually do the work of talking to postgres in all cases. diesel-async can use
tokio-postgres to execute queries. refinery can use tokio-postgres to run migrations. deadpool can
pool tokio-postgres connections.

I've chosen this stack specifically to avoid depending on `libpq`, the c implementation of a
postgres client. This is not because it's written in C. 99.9% of postgres client libraries use libpq
to great success. It is to keep the build process for pict-rs simple. Sticking with a full stack of
rust means that only a rust compiler is required to build pict-rs.


## Plan

pict-rs isolates different concepts between different "Repo" traits. There's a single top-level
FullRepo that depends on the others to ensure everything gets implemented properly. Since there's
only been one repo implementation so far, it's not optimized for network databases and some things
are less efficient than they could be.


### HashRepo
This is a little complicated because one of the things a HashRepo can do is return a stream of
hashes from the repo. This can likely be implemented as a batch-retrieval operation that fetches
1000 hashes at once and then drains them on each call to `poll_next`

methods:
- size
- hashes
- hash_page
- hash_page_by_date
- bound
- create_hash
- create_hash_with_timestamp
- update_identifier
- identifier
- relate_variant_identifier
- variant_identifier
- variants
- remove_variant
- relate_motion_identifier
- motion_identifier
- cleanup_hash

```sql
CREATE TABLE hashes (
    hash BYTEA PRIMARY KEY,
    identifer TEXT NOT NULL,
    motion_identifier TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- paging through hashes
CREATE INDEX ordered_hash_index ON hashes (created_at, hash);


CREATE TABLE variants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    hash BYTEA REFERENCES hashes(hash) ON DELETE CASCADE,
    variant TEXT NOT NULL,
    identifier TEXT NOT NULL
);


CREATE UNIQUE INDEX hash_variant_index ON variants (hash, variant);
```


### AliasRepo
Used to relate Aliases to Hashes, and to relate Delete Tokens to Aliases. Hashes are always bytes,
but could be hex-encoded. postgres's `bytea` type can work with hex-encoding on storage and
retrieval so maybe this can be used. Delete Tokens are not always UUIDs, even though they have been
UUIDs in all recent versions of pict-rs.

methods:
- create_alias
- delete_token
- hash
- aliases_for_hash
- cleanup_alias

```sql
CREATE TABLE aliases (
    alias VARCHAR(50) PRIMARY KEY,
    hash BYTEA NOT NULL REFERENCES hashes(hash) ON DELETE CASCADE,
    delete_token VARCHAR(30) NOT NULL
);


CREATE INDEX aliases_hash_index ON aliases (hash);
```


### SettingsRepo
This is used for generic server-level storage. The file & object stores keep their current path
generator values here. This is also used in some migrations to mark completion.

methods:
- set
- get
- remove

could be a simple table with String key & values

pict-rs currently treats the value here as Bytes, so it could either be hex-encoded or translated to
a string
```sql
CREATE TABLE settings (
    key VARCHAR(80) PRIMARY KEY,
    value VARCHAR(80) NOT NULL
);
```


### DetailsRepo
Used to relate details (image metadata) to identifiers (image paths). Identifiers are currently
treated as bytes, so may need hex-encoding to store in the database. They _should_ be valid strings
in most environments, so it might be possible to drop the bytes requirement & instead have a string
requirement.

methods:
- relate_details
- details
- cleanup_details

```sql
CREATE TABLE details (
    identifier TEXT PRIMARY KEY,
    details JSONB NOT NULL,
);
```


### QueueRepo
This is going to be the troublesome table. It represents jobs that will be processed. Jobs are
pushed as Bytes, but at a higher level are actually JSON strings. The QueueRepo API could be updated
to take `T: Serialize` as input rather than bytes, and then we can store it as JSONB. With the
current API, the repo doesn't need to know the shape of a job, and maybe that is a benefit. We
should take care in the future not to query on the contents of the job.

methods:
- push
- pop
- heartbeat
- complete_job

```sql
CREATE TYPE job_status AS ENUM ('new', 'running');


CREATE TABLE job_queue (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    queue VARCHAR(30) NOT NULL,
    job JSONB NOT NULL,
    status job_status NOT NULL DEFAULT 'new',
    queue_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    heartbeat TIMESTAMP
);


CREATE INDEX queue_status_index ON queue INCLUDE queue, status;
CREATE INDEX heartbeat_index ON queue INCLUDE heartbeat;
```

claiming a job can be
```sql
UPDATE job_queue SET status = 'new', heartbeat = NULL
WHERE
    heartbeat IS NOT NULL AND heartbeat < NOW - INTERVAL '2 MINUTES';

UPDATE job_queue SET status = 'running', heartbeat = CURRENT_TIMESTAMP
WHERE id = (
    SELECT id
    FROM job_queue
    WHERE status = 'new' AND queue = '$QUEUE'
    ORDER BY queue_time ASC
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
returning *;
```

notifying pict-rs of a ready job could be
```sql
CREATE OR REPLACE FUNCTION queue_status_notify()
	RETURNS trigger AS
$$
BEGIN
	PERFORM pg_notify('queue_status_channel', NEW.id::text);
	RETURN NEW;
END;
$$ LANGUAGE plpgsql;


CREATE TRIGGER queue_status
	AFTER INSERT OR UPDATE OF status
	ON queue
	FOR EACH ROW
EXECUTE PROCEDURE queue_status_notify();
```

Postgres queue implementation from this blog post: https://webapp.io/blog/postgres-is-the-answer/


### StoreMigrationRepo
This is used for migrating from local storage to object storage. It keeps track of which identifiers
have been migrated, and on a successful migration, it is fully cleared.

methods:
- is_continuing_migration
- mark_migrated
- is_migrated
- clear

```sql
CREATE TABLE migrations (
    identifier TEXT PRIMARY KEY,
);
```


### ProxyRepo
This is used for keeping track of URLs that map to Aliases for proxied media.

methods:
- relate_url
- related
- remove_relation

```sql
CREATE TABLE proxies (
    url PRIMARY KEY,
    alias VARCHAR(30) NOT NULL REFERENCES aliases(alias)
);
```


### AliasAccessRepo
This is used for keeping track of aliases that are "cached" in pict-rs and can be safely removed
when they are not useful to keep around. This might be able to piggyback on the aliases table or the
proxies table.

methods:
- accessed_alias
- set_accessed_alias
- alias_accessed_at
- older_aliases
- remove_alias_access

```sql
ALTER TABLE aliases ADD COLUMN accessed TIMESTAMP;
```
or
```sql
ALTER TABLE proxies ADD COLUMN accessed TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP;
```


### VariantAccessRepo
This is used for keeping track of access times for variants of an image to enable freeing up space
from seldom-accessed variants. This might be able to piggyback on the variants table.

methods:
- accessed_variant
- set_accessed_variant
- variant_accessed_at
- older_variants
- remove_variant_access

```sql
ALTER TABLE variants ADD COLUMN accessed TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP;
```

### UploadRepo
Used to keep track of backgrounded uploads.

methods:
- create_upload
- wait
- claim
- complete_upload

```sql
CREATE TABLE uploads (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    result JSONB,
);
```

Waiting for an upload
```sql
CREATE OR REPLACE FUNCTION upload_completion_notify()
	RETURNS trigger AS
$$
BEGIN
	PERFORM pg_notify('upload_completion_channel', NEW.id::text);
	RETURN NEW;
END;
$$ LANGUAGE plpgsql;


CREATE TRIGGER upload_result
	AFTER INSERT OR UPDATE OF result
	ON uploads
	FOR EACH ROW
EXECUTE PROCEDURE upload_completion_notify();
```
