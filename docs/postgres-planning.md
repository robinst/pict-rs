# Planning for implementing a postgres repo for pict-rs

FullRepo currently depends on a number of specific repo traits Many of these are simple key -> value
mappings that can be done with columns or tables, but some are more complicated (like background job
processing).

Some of the existing repo methods could be consolidated to better represent the data and operations
that need to be performed. This should happen _before_ impelementing the postgres repo.


### HashRepo
This is a little complicated because one of the things a HashRepo can do is return a stream of
hashes from the repo. This can likely be implemented as a batch-retrieval operation that fetches
1000 hashes at once and then drains them on each call to `poll_next`

This is also probably made up of multiple tables, and can reuse some other tables. It's likely that
some of this functionality can be consolidated with AliasRepo. In both usages, relate_hash and
relate_alias are called together.

Create could also maybe be updated to take the identifier as an argument, since it doesn't make
sense to have a hash without an identifier. This might affect the ingest process' ordering

methods:
- size
- hashes
- create
- relate_alias
- remove_alias
- aliases
- relate_identifier
- relate_variant_identifier
- variant_identifier
- remove_variant
- relate_motion_identifier
- cleanup

```sql
CREATE TABLE hashes (
    hash BYTEA PRIMARY KEY,
    identifer TEXT NOT NULL,
    motion_identifier TEXT,
);

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
- create
- relate_delete_token
- relate_hash
- hash
- cleanup

This can probably be simplified. If `create` took a `hash` as an argument and returned a
`delete_token` we could have all of our required information up-front and avoid the hassle of
generating each part of this separately

```sql
CREATE TABLE aliases (
    alias VARCHAR(30) PRIMARY KEY,
    hash BYTEA NOT NULL REFERENCES hashes(hash) ON DELETE CASCADE,
    delete_token VARCHAR(30) NOT NULL
);
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


### IdentifierRepo
Used to relate details (image metadata) to identifiers (image paths). Identifiers are currently
treated as bytes, so may need hex-encoding to store in the database. They _should_ be valid strings
in most environments, so it might be possible to drop the bytes requirement & instead have a string
requirement.

methods:
- relate_details
- details
- cleanup

```sql
CREATE TABLE details (
    identifier TEXT PRIMARY KEY,
    details JSONB NOT NULL,
);
```


### QueueRepo
This is going to be the troublesome table. It represents jobs that will be processed. Jobs are
pushed as Bytes, but at a higher level are actually JSON strings. The QueueRepo API could be updated
to take `T: Serizlie` as input rather than bytes, and then we can store it as JSONB. With the
current API, the repo doesn't need to know the shape of a job, and maybe that is a benefit. We
should take care in the future not to query on the contents of the job.

methods:
- requeue_in_progress
- push
- pop

```sql
CREATE TYPE job_status AS ENUM ('new', 'running');

CREATE TABLE queue (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    queue VARCHAR(30) NOT NULL,
    job JSONB NOT NULL,
    worker_id VARCHAR(30),
    status job_status NOT NULL DEFAULT 'new',
    queue_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX queue_worker_id_index ON queue INCLUDE worker_id;
CREATE INDEX queue_status_index ON queue INCLUDE status;
```

claiming a job can be
```sql
DELETE FROM queue WHERE worker_id = '$WORKER_ID';

UPDATE queue SET status = 'running', worker_id = '$WORKER_ID'
WHERE id = (
    SELECT id
    FROM queue
    WHERE status = 'new'
    ORDER BY queue_time ASC
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
returning *;
```


### MigrationRepo
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
- accessed
- older_aliases
- remove_access

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
- accessed
- contains_variant
- older_variants
- remove_access

```sql
ALTER TABLE variants ADD COLUMN accessed TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP;
```
