# pict-rs
_a simple image hosting service_

## Navigation
1. [Links](#links)
2. [Usage](#usage)
    1. [Running](#running)
        1. [Commandline](#commandline)
        2. [Docker](#docker)
        3. [Bare Metal](#bare-metal)
            1. [Distro Package](#distro-package)
            2. [Binary Download](#binary-download)
            3. [Compile from Source](#compile-from-source)
    2. [Api](#api)
3. [Administration](#administration)
    1. [Backups](#backups)
    2. [0.3 to 0.4 Migration Guide](#0-3-to-0-4-migration-guide)
    3. [Filesystem to Object Storage Migration](#filesystem-to-object-storage-migration)
        1. [Troubleshooting](#migration-troubleshooting)
4. [Development](#development)
    1. [Nix Development](#nix-development)
        1. [With direnv and nix-direnv](#with-direnv-and-nix-direnv)
        2. [With just Nix](#with-just-nix)
    2. [Docker Development](#docker-development)
        1. [With Arch](#with-arch)
        2. [With Alpine](#with-alpine)
5. [Contributing](#contributing)
6. [FAQ](#faq)
    1. [Is pict-rs stateless?](#question-is-pict-rs-stateless)
    2. [Can I use a different database?](#question-can-i-use-a-different-database-with-pict-rs)
    3. [How can I submit changes?](#question-how-can-i-submit-changes)
    4. [I want to configure with $format](#question-i-want-to-configure-it-with-yaml-instead-of-toml)
    5. [How do I donate?](#question-how-do-i-donate-to-pict-rs)
7. [Common Problems](#common-problems)
8. [License](#license)

## Links
- Find the code on [gitea](https://git.asonix.dog/asonix/pict-rs)
- Join the discussion on [matrix](https://matrix.to/#/#pictrs:matrix.asonix.dog?via=matrix.asonix.dog)
- Hit me up on [mastodon](https://masto.asonix.dog/@asonix)

## Usage
### Running
#### Commandline
```
$ pict-rs -h
A simple image hosting service

Usage: pict-rs [OPTIONS] <COMMAND>

Commands:
  run            Runs the pict-rs web server
  migrate-store  Migrates from one provided media store to another
  help           Print this message or the help of the given subcommand(s)

Options:
  -c, --config-file <CONFIG_FILE>
          Path to the pict-rs configuration file
      --old-db-path <OLD_DB_PATH>
          Path to the old pict-rs sled database
      --log-format <LOG_FORMAT>
          Format of logs printed to stdout [possible values: compact, json, normal, pretty]
      --log-targets <LOG_TARGETS>
          Log levels to print to stdout, respects RUST_LOG formatting
      --console-address <CONSOLE_ADDRESS>
          Address and port to expose tokio-console metrics
      --console-buffer-capacity <CONSOLE_BUFFER_CAPACITY>
          Capacity of the console-subscriber Event Buffer
      --opentelemetry-url <OPENTELEMETRY_URL>
          URL to send OpenTelemetry metrics
      --opentelemetry-service-name <OPENTELEMETRY_SERVICE_NAME>
          Service Name to use for OpenTelemetry
      --opentelemetry-targets <OPENTELEMETRY_TARGETS>
          Log levels to use for OpenTelemetry, respects RUST_LOG formatting
      --save-to <SAVE_TO>
          File to save the current configuration for reproducible runs
  -h, --help
          Print help information
  -V, --version
          Print version information
```

```
$ pict-rs run -h
Runs the pict-rs web server

Usage: pict-rs run [OPTIONS] [COMMAND]

Commands:
  filesystem      Run pict-rs with filesystem storage
  object-storage  Run pict-rs with object storage
  help            Print this message or the help of the given subcommand(s)

Options:
  -a, --address <ADDRESS>
          The address and port to bind the pict-rs web server
      --api-key <API_KEY>
          The API KEY required to access restricted routes
      --worker-id <WORKER_ID>
          ID of this pict-rs node. Doesn't do much yet
      --media-preprocess-steps <MEDIA_PREPROCESS_STEPS>
          Optional pre-processing steps for uploaded media
      --media-skip-validate-imports <MEDIA_SKIP_VALIDATE_IMPORTS>
          Whether to validate media on the "import" endpoint [possible values: true, false]
      --media-max-width <MEDIA_MAX_WIDTH>
          The maximum width, in pixels, for uploaded media
      --media-max-height <MEDIA_MAX_HEIGHT>
          The maximum height, in pixels, for uploaded media
      --media-max-area <MEDIA_MAX_AREA>
          The maximum area, in pixels, for uploaded media
      --media-max-file-size <MEDIA_MAX_FILE_SIZE>
          The maximum size, in megabytes, for uploaded media
      --media-max-frame-count <MEDIA_MAX_FRAME_COUNT>
          The maximum number of frames allowed for uploaded GIF and MP4s
      --media-enable-silent-video <MEDIA_ENABLE_SILENT_VIDEO>
          Whether to enable GIF and silent video uploads [possible values: true, false]
      --media-enable-full-video <MEDIA_ENABLE_FULL_VIDEO>
          Whether to enable full video uploads [possible values: true, false]
      --media-video-codec <MEDIA_VIDEO_CODEC>
          Enforce a specific video codec for uploaded videos [possible values: h264, h265, av1, vp8, vp9]
      --media-audio-codec <MEDIA_AUDIO_CODEC>
          Enforce a specific audio codec for uploaded videos [possible values: aac, opus, vorbis]
      --media-filters <MEDIA_FILTERS>
          Which media filters should be enabled on the `process` endpoint
      --media-format <MEDIA_FORMAT>
          Enforce uploaded media is transcoded to the provided format [possible values: avif, jpeg, jxl, png, webp]
  -h, --help
          Print help information (use `--help` for more detail)
```

Try running `help` commands for more runtime configuration options
```bash
$ pict-rs run filesystem -h
$ pict-rs run object-storage -h
$ pict-rs run filesystem sled -h
$ pict-rs run object-storage sled -h
```

See [`pict-rs.toml`](./pict-rs.toml) for more
configuration

##### Example:
Run with the default configuration
```bash
$ ./pict-rs run
```
Running on all interfaces, port 8080, storing data in /opt/data
```bash
$ ./pict-rs \
    run -a 0.0.0.0:8080 \
    filesystem -p /opt/data/files \
    sled -p /opt/data/sled-repo
```
Running locally, port 9000, storing data in data/, and converting all uploads to PNG
```bash
$ ./pict-rs \
    run \
        -a 127.0.0.1:9000 \
        --media-format png \
    filesystem -p data/files \
    sled -p data/sled-repo
```
Running locally, port 8080, storing data in data/, and only allowing the `thumbnail` and `identity` filters
```bash
$ ./pict-rs \
    run \
        -a 127.0.0.1:8080 \
        --media-filters thumbnail \
        --media-filters identity \
    filesystem -p data/files \
    sled -p data/sled-repo
```
Running from a configuration file
```bash
$ ./pict-rs -c ./pict-rs.toml run
```
Migrating to object storage from filesystem storage. For more detailed info, see
[Filesystem to Object Storage Migration](#filesystem-to-object-storage-migration)
```bash
$ ./pict-rs \
    migrate-store \
    filesystem -p data/files \
    object-storage \
        -a ACCESS_KEY \
        -b BUCKET_NAME \
        -r REGION \
        -s SECRET_KEY
```
Dumping configuration overrides to a toml file
```bash
$ ./pict-rs --save-to pict-rs.toml \
    run \
    object-storage \
        -a ACCESS_KEY \
        -b pict-rs \
        -r us-east-1 \
        -s SECRET_KEY \
    sled -p data/sled-repo
```

#### Docker
Run the following commands:
```bash
# Create a folder for the files (anywhere works)
$ mkdir ./pict-rs
$ cd ./pict-rs
$ mkdir -p volumes/pictrs
$ sudo chown -R 991:991 volumes/pictrs
$ wget https://git.asonix.dog/asonix/pict-rs/raw/branch/main/docker/prod/docker-compose.yml
$ sudo docker-compose up -d
```
###### Note
- pict-rs makes use of the system's temporary folder. This is generally `/tmp` on linux
- pict-rs makes use of an imagemagick security policy at
    `/usr/lib/ImageMagick-$VERSION/config-Q16HDRI/policy.xml`

#### Bare Metal
There are a few options for acquiring pict-rs to run outside of docker.
1. Packaged via your distro of choice
2. Binary download from [the releases page](https://git.asonix.dog/asonix/pict-rs/tags)
3. Compiled from source

If running outside of docker, the recommended configuration method is via the
[`pict-rs.toml`](./pict-rs.toml) file. When running pict-rs, the file can be passed to the binary as
a commandline argument.
```bash
$ pict-rs -c /path/to/pict-rs.toml run
```

##### Distro Package
If getting pict-rs from your distro, please make sure it's a recent version (meaning 0.3.x stable,
or 0.4.0-rc.x). If it is older, consider using an alternative option for installing pict-rs. I am
currently aware of pict-rs packaged in [the AUR](https://aur.archlinux.org/packages/pict-rs) and
[nixpkgs](https://search.nixos.org/packages?channel=23.05&from=0&size=50&sort=relevance&type=packages&query=pict-rs),
but there may be other distros that package it as well.

##### Binary Download
pict-rs provides precompiled binaries that should work on any linux system for x86_64, aarch64, and
armv7h on [the releases page](https://git.asonix.dog/asonix/pict-rs/tags). If downloading a binary,
make sure that you have the following dependencies installed:
- `imagemagick` 7
- `ffmpeg` 5 or 6
- `exiftool` 12 (sometimes called `perl-image-exiftool`)

These binaries are called by pict-rs to process uploaded media, so they must be in the `$PATH`
available to pict-rs.

##### Compile from Source
pict-rs can be compiled from source using a recent version of the rust compiler. I do development
and produce releases on 1.70. pict-rs also requires the `protoc` protobuf compiler to be present at
build-time in order to enable use of [`tokio-console`](https://github.com/tokio-rs/console).

Like the Binary Download option, `imagemagick`, `ffmpeg`, and `exiftool` must be installed for
pict-rs to run properly.


### API
pict-rs offers the following endpoints:
- `POST /image` for uploading an image. Uploaded content must be valid multipart/form-data with an
    image array located within the `images[]` key

    This endpoint returns the following JSON structure on success with a 201 Created status
    ```json
    {
        "files": [
            {
                "delete_token": "JFvFhqJA98",
                "file": "lkWZDRvugm.jpg",
                "details": {
                    "width": 800,
                    "height": 800,
                    "content_type": "image/jpeg",
                    "created_at": "2022-04-08T18:33:42.957791698Z"
                }
            },
            {
                "delete_token": "kAYy9nk2WK",
                "file": "8qFS0QooAn.jpg",
                "details": {
                    "width": 400,
                    "height": 400,
                    "content_type": "image/jpeg",
                    "created_at": "2022-04-08T18:33:42.957791698Z"
                }
            },
            {
                "delete_token": "OxRpM3sf0Y",
                "file": "1hJaYfGE01.jpg",
                "details": {
                    "width": 400,
                    "height": 400,
                    "content_type": "image/jpeg",
                    "created_at": "2022-04-08T18:33:42.957791698Z"
                }
            }
        ],
        "msg": "ok"
    }
    ```
- `POST /image/backgrounded` Upload an image, like the `/image` endpoint, but don't wait to validate and process it.
    This endpoint returns the following JSON structure on success with a 202 Accepted status
    ```json
    {
        "uploads": [
            {
                "upload_id": "c61422e1-9294-4f1f-977f-c696b7939467",
            },
            {
                "upload_id": "62cc707f-725c-44b6-908f-2bd8946c3c29"
            }
        ],
        "msg": "ok"
    }
    ```
- `GET /image/download?url={url}&backgrounded=(true|false)` Download an image
    from a remote server, returning the same JSON payload as the `POST /image` endpoint by default.

    if `backgrounded` is set to `true`, then the ingest processing will be queued for later and the
    response json will be the same as the `POST /image/backgrounded` endpoint.
- `GET /image/backgrounded/claim?upload_id={uuid}` Wait for a backgrounded upload to complete, claiming it's result
    Possible results:
    - 200 Ok (validation and ingest complete):
        ```json
        {
            "files": [
                {
                    "delete_token": "OxRpM3sf0Y",
                    "file": "1hJaYfGE01.jpg",
                    "details": {
                        "width": 400,
                        "height": 400,
                        "content_type": "image/jpeg",
                        "created_at": "2022-04-08T18:33:42.957791698Z"
                    }
                }
            ],
            "msg": "ok"
        }
        ```
    - 422 Unprocessable Entity (validation or otherwise failure):
        ```json
        {
            "msg": "Error message about what went wrong with upload"
        }
        ```
    - 204 No Content (Upload validation and ingest is not complete, and waiting timed out)
        In this case, trying again is fine
- `GET /image/original/{file}` for getting a full-resolution image. `file` here is the `file` key from the
    `/image` endpoint's JSON
- `GET /image/details/original/{file}` for getting the details of a full-resolution image.
    The returned JSON is structured like so:
    ```json
    {
        "width": 800,
        "height": 537,
        "content_type": "image/webp",
        "created_at": "2022-04-08T18:33:42.957791698Z"
    }
    ```
- `GET /image/process.{ext}?src={file}&...` get a file with transformations applied.
    existing transformations include
    - `identity=true`: apply no changes
    - `blur={float}`: apply a gaussian blur to the file
    - `thumbnail={int}`: produce a thumbnail of the image fitting inside an `{int}` by `{int}`
        square using raw pixel sampling
    - `resize={int}`: produce a thumbnail of the image fitting inside an `{int}` by `{int}` square
        using a Lanczos2 filter. This is slower than sampling but looks a bit better in some cases
    - `resize={filter}.(a){int}`: produce a thumbnail of the image fitting inside an `{int}` by
        `{int}` square, or when `(a)` is present, produce a thumbnail whose area is smaller than
        `{int}`. `{filter}` is optional, and indicates what filter to use when resizing the image.
        Available filters are `Lanczos`, `Lanczos2`, `LanczosSharp`, `Lanczos2Sharp`, `Mitchell`,
        and `RobidouxSharp`.

        Examples:
        - `resize=300`: Produce an image fitting inside a 300x300 px square
        - `reizie=.a10000`: Produce an image whose area is at most 10000 px
        - `resize=Mitchell.200`: Produce an image fitting inside a 200x200 px square using the
            Mitchell filter
        - `resize=RobidouxSharp.a40000`: Produce an image whose area is at most 40000 px using the
            RobidouxSharp filter
    - `crop={int-w}x{int-h}`: produce a cropped version of the image with an `{int-w}` by `{int-h}`
        aspect ratio. The resulting crop will be centered on the image. Either the width or height
        of the image will remain full-size, depending on the image's aspect ratio and the requested
        aspect ratio. For example, a 1600x900 image cropped with a 1x1 aspect ratio will become 900x900. A
        1600x1100 image cropped with a 16x9 aspect ratio will become 1600x900.

    Supported `ext` file extensions include `avif`, `jpg`, `jxl`, `png`, and `webp`

    An example of usage could be
    ```
    GET /image/process.jpg?src=asdf.png&thumbnail=256&blur=3.0
    ```
    which would create a 256x256px JPEG thumbnail and blur it
- `GET /image/process_backgrounded.{ext}?src={file}&...` queue transformations to be applied to a given file. This accepts the same arguments as the `process.{ext}` endpoint, but does not wait for the processing to complete.
- `GET /image/details/process.{ext}?src={file}&...` for getting the details of a processed image.
    The returned JSON is the same format as listed for the full-resolution details endpoint.
- `DELETE /image/delete/{delete_token}/{file}` or `GET /image/delete/{delete_token}/{file}` to
    delete a file, where `delete_token` and `file` are from the `/image` endpoint's JSON


The following endpoints are protected by an API key via the `X-Api-Token` header, and are disabled
unless the `--api-key` option is passed to the binary or the PICTRS__SERVER__API_KEY environment variable is
set.

A secure API key can be generated by any password generator.
- `POST /internal/import` for uploading an image while preserving the filename as the first alias.
    The upload format and response format are the same as the `POST /image` endpoint.
- `POST /internal/purge?alias={alias}` Purge a file by it's alias. This removes all aliases and
    files associated with the query.

    This endpoint returns the following JSON
    ```json
    {
        "msg": "ok",
        "aliases": ["asdf.png"]
    }
    ```
- `GET /internal/aliases?alias={alias}` Get the aliases for a file by it's alias

    This endpiont returns the same JSON as the purge endpoint
- `DELETE /internal/variants` Queue a cleanup for generated variants of uploaded images.

    If any of the cleaned variants are fetched again, they will be re-generated.
- `GET /internal/identifier` Get the image identifier (file path or object path) for a given alias

    On success, the returned json should look like this:
    ```json
    {
        "msg": "ok",
        "identifier": "/path/to/object"
    }
    ```
- `POST /internal/set_not_found` Set the 404 image that is served from the original and process
    endpoints. The image used must already be uploaded and have an alias. The request should look
    like this:
    ```json
    {
        "alias": "asdf.png"
    }
    ```

    On success, the returned json should look like this:
    ```json
    {
        "msg": "ok"
    }
    ```

Additionally, all endpoints support setting deadlines, after which the request will cease
processing. To enable deadlines for your requests, you can set the `X-Request-Deadline` header to an
i128 value representing the number of nanoseconds since the UNIX Epoch. A simple way to calculate
this value is to use the `time` crate's `OffsetDateTime::unix_timestamp_nanos` method. For example,
```rust
// set deadline of 1ms
let deadline = time::OffsetDateTime::now_utc() + time::Duration::new(0, 1_000);

let request = client
    .get("http://pict-rs:8080/image/details/original/asdfghjkla.png")
    .insert_header(("X-Request-Deadline", deadline.unix_timestamp_nanos().to_string())))
    .send()
    .await;
```


## Administration
### Backups
pict-rs maintains two folders that matter: the `sled-repo` directory, and the `files` directory.
`sled-repo` is where it keeps metadata about files such as: their location, their aliases, their
processed versions' locations, their dimensions, mime type, etc. `files` is where it puts uploaded
files when storing on the local filesystem.

The `sled-repo` folder is generally small compared to the `files` folder, and backing it up can be
as simple as copying the folder somewhere else. I recommend doing so while pict-rs is not running.

### 0.3 to 0.4 Migration Guide
pict-rs will automatically migrate from the 0.3 db format to the 0.4 db format on the first launch
of 0.4. If you are running the provided docker container without any custom configuration, there are
no additional steps.

If you have any custom configuration for file paths, or you are running outside of docker, then
there is some extra configuration that needs to be done.

If your previous `PICTRS__PATH` variable or `path` config was set, it needs to be translated to the
new configuration format.

`PICTRS_PATH` has split into three separate config options:
- `PICTRS__OLD_DB__PATH`: This should be set to the same value that `PICTRS__PATH` was. It is used
    during the migration from 0.3 to 0.4
- `PICTRS__REPO__PATH`: This is the location of the 0.4 database. It should be set to a subdirectory
    of the previous `PICTRS__PATH` directory. I would recommend `/previous/path/sled-repo`
- `PICTRS__STORE__PATH`: This is the location of the files. It should be the `files` subdirectory of
    the previous PICTRS__PATH directory.

if you configured via the configuration file, these would be
```toml
[old_db]
path = "/previous/path"

[repo]
path = "/previous/path/sled-repo"

[store]
path = "/previous/path/files"
```

If your previous `RUST_LOG` variable was set, it has been split into two different configuration
options:
- `PICTRS__TRACING__LOGGING__TARGETS`: This dictates what logs should be printed in the console while
    pict-rs is running.
- `PICTRS__TRACING__OPENTELEMETRY__TARGETS`: This dictates what spans and events should be exported
    as opentelemetry data, if enabled.

You can also configure these options via the configuration file:
```toml
[tracing.logging]
targets = "debug"

[tracing.opentelemetry]
targets = "debug"
```

If the migration doesn't work due to a configuration error, the new sled-repo directory can be
deleted and a new migration will be automatically triggered on the next launch.


### Filesystem to Object Storage migration
_Make sure you take a backup of the sled-repo directory before running this command!!! Migrating to
object storage updates the database and if you need to revert for any reason, you'll want a backup._

After migrating from 0.3 to 0.4, it is possible to migrate to object storage. This can be useful if
hosting in a cloud environment, since object storage is generally far cheaper than block storage.

There's a few required configuration options for object storage. I will try to explain:
- endpoint: this is the URL at which the object storage is available. Generally this URL will look
    like `https://<bucket-name>.<region>.s3.example.com`, but sometimes it could look like
    `https://<region>.s3.example.com` or just `https://s3.example.com`
- bucket-name: this is name of the "bucket" in which the objects will reside. A bucket must already
    exist for the migration to work - pict-rs will not create the bucket on it's own. It is up to
    you to create a bucket with your storage provider ahead of time.
- region: this is the "location" in which your bucket resides. It may not be meaningful depending on
    your cloud provider, but it is always required.
- access-key: this is a secret your cloud provider will give to you in order to access the bucket
- secret-key: this is a second secret your cloud provider will give to you in order to access the
    bucket

The command will look something like this:
```bash
$ pict-rs \
    migrate-store \
    filesystem \
        -p /path/to/files \
    object-storage \
        -e https://object-storage-endpoint \
        -b bucket-name \
        -r region \
        -a access-key \
        -s secret-key \
    sled \
        -p /path/to/sled-repo
```

If you are running the docker container with default paths, it can be simplified to the following:
```bash
$ pict-rs \
    migrate-store \
    filesystem \
    object-storage \
        -e https://object-storage-endpoint \
        -b bucket-name \
        -r region \
        -a access-key \
        -s secret-key
```

_This command must be run while pict-rs is offline._

If you're running with docker-compose, this could look like the following:
```bash
$ sudo docker compose stop pictrs # stop the pict-rs container
$ sudo docker compose run pictrs sh # launch a shell in the pict-rs container
> pict-rs --version # verify pict-rs version is recent (should probably be 0.4.0-rc.13 or later)
> pict-rs \
    migrate-store \
    filesystem \
    object-storage \
        -e endpoint \
        -b bucket \
        -r region \
        -a -access-key \
        -s secret-key
> exit
$ vi docker-compose.yml # edit the docker-compose yaml however you like to edit it, make sure all the variables described below are set
$ sudo docker compose up -d pictrs # start pict-rs again after the migration. Note that this is not 'docker compose start'. using the `up` subcommand explicitly reloads configurations
```
depending on your version of docker or docker-compose, you might need to use the following command to open a shell:
```bash
$ sudo docker-compose run -i pictrs sh
```

Here's an example based on my own object storage that I host myself on kubernetes with
[`garage`](https://garagehq.deuxfleurs.fr/):
```bash
$ pict-rs \
    migrate-store \
    filesystem \
    object-storage \
        --use-path-style \
        -e http://garage-daemon.garage.svc:3900 \
        -b pict-rs \
        -r garage \
        -a <redacted> \
        -s <redacted>
```

Here's an example based on a backblaze b2 user's configuration;
```bash
$ pict-rs \
    migrate-store \
    filesystem \
    object-storage \
        --use-path-style \
        -e https://s3.us-east-005.backblazeb2.com \
        -r us-east-005 \
        -b SitenamePictrs \
        -a redacted \
        -s redacted
```

After you've completed the migration, update your pict-rs configuration to use object storage. If
you configure using environment variables, make sure the following are set:
- `PICTRS__STORE__TYPE=object_storage`
- `PICTRS__STORE__ENDPOINT=https://object-storage-endpoint`
- `PICTRS__STORE__BUCKET_NAME=bucket-name`
- `PICTRS__STORE__REGION=region`
- `PICTRS__STORE__USE_PATH_STYLE=false` (set to true if your object storage requires path style access)
- `PICTRS__STORE__ACCESS_KEY=access-key`
- `PICTRS__STORE__SECRET_KEY=secret-key`

If you use the configuration file, this would be
```toml
[store]
type = "object_storage"
endpoint = "https://object-storage-endpoint"
bucket_name = "bucket-name"
region = "region"
use_path_style = false # Set to true if your object storage requires path style access
access_key = "access-key"
secret_key = "secret-key"
```

#### Migration Troubleshooting
If you see an error while trying to launch the migration that looks like this:
```
   0: IO error: could not acquire lock on "/mnt/sled-repo/v0.4.0-alpha.1/db": Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
```
This means that pict-rs could not open it's database. This is probably because another copy of
pict-rs is currently running. Make sure to stop all running pict-rs processes before migrating.

If you are trying to migrate and seeing "Failed moving file. Retrying +1", Do not cancel the
migration. Let it reach 10 retries. It will print a more meaningful error afterwards. Here are some
examples of errors and their casuses:

Error:
```
   0: Error in store
   1: Error in object store
   2: Invalid status: 400 Bad Request
   2: <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
      <Error>
          <Code>InvalidRequest</Code>
          <Message>Authorization header's Credential is malformed</Message>
      </Error>
```
Cause: the region was set improperly. Additionaly a path-style endpoint was used without passing
`--use-path-style`

Error:
```
   0: Error in store
   1: Error in object store
   2: Invalid status: 403 Forbidden
   2: <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
      <Error>
          <Code>InvalidAccessKeyId</Code>
          <Message>Malformed Access Key Id</Message>
      </Error>
```
Cause: the access key was set improperly


If you have enabled object storage without first migrating your existing files to object storage,
these migrate commands may end up retrying file migrations indefinitely. In order to successfully
resolve this multi-store problem the `--skip-missing-files` flag has been added to the
`migrate-store` subcommand. This tells pict-rs not to retry migrating a file if that file returns
some form of "not found" error.

```bash
$ pict-rs \
    migrate-store --skip-missing-files \
    filesystem -p /path/to/files \
    object-storage \
        -e https://object-storage-endpoint \
        -b bucket-name \
        -r region \
        -a access-key \
        -s secret-key \
    sled \
        -p /path/to/sled-repo
```

If you have trouble getting pict-rs to upload to your object storage, check a few things:
Does your object storage require path-style access? Some object storage providers, like Contabo,
don't support virtual hosted buckets. Here's a basic example:

Path style URL: `https://region.example.com/bucket-name`
Virtual host style URL: `https://bucket-name.region.example.com`

If you do need to use path style, your command might look like this:
```
$ pict-rs \
    migrate-store \
    filesystem -p /path/to/files \
    object-storage \
        --use-path-style \
        -e https://object-storage-endpoint \
        -b bucket-name \
        -r region \
        -a access-key \
        -s secret-key \
    sled \
        -p /path/to/sled-repo
```

Additionally, some providers might require you include the `region` in your endpoint URL:
`https://region.host.com`, while others might just require a top-level endpoint:
`https://example.com`.

Check your object storage provider's documentation to be sure you're setting the right values.


## Development
pict-rs has a few native dependencies that need to be installed in order for it to run properly.
Currently these are as follows:

- imagemagick 7.1.1 (although 7.0 and 7.1.0 may also work)
- ffmpeg 6 (although 5 may also work)
- exiftool 12.62 (although 12.50 or newer may also work)

Additionally, pict-rs requires a protobuf compiler during the compilation step to support
tokio-console, a runtime debug tool.

Installing these from your favorite package manager should be sufficient. Below are some fun ways to
develop and test a pict-rs binary.

### Nix Development
I personally use nix for development. The provided [`flake.nix`](./flake.nix) file should be
sufficient to create a development environment for pict-rs on any linux distribution, provided nix
is installed.

#### With direnv and nix-direnv
With these tools, the pict-rs development environment can be automatically loaded when entering the pict-rs directory

Setup (only once):
```
$ echo 'use flake' > .envrc
$ direnv allow
```

Running:
```
$ cargo run -- -c dev.toml run
```
#### With just Nix
```
$ nix develop
$ cargo run -- -c dev.toml run
```

### Docker Development
Previously, I have run pict-rs from inside containers that include the correct dependencies. The two
options listed below are ones I have personally tried.

#### With Arch
This option doesn't take much configuration, just compile the binary and run it from inside the container

```bash
$ cargo build
$ sudo docker run --rm -it -p 8080:8080 -v "$(pwd):/mnt" archlinux:latest
> pacman -Syu imagemagick ffmepg perl-image-exiftool
> cp /mnt/docker/prod/root/usr/lib/ImageMagick-7.1.1/config-Q16HDRI/policy.xml /usr/lib/ImageMagick-7.1.1/config-Q16HDRI/
> PATH=$PATH:/usr/bin/vendor_perl /mnt/target/debug/pict-rs --log-targets debug run
```

#### With Alpine
This option requires `cargo-zigbuild` to be installed. Cargo Zigbuild is a tool that links rust
binaries with Zig's linker, enabling easy cross-compiles to many targets. Zig has put a lot of
effort into seamless cross-compiling, and it is nice to be able to take advantage of that work from
rust.

```bash
$ cargo zigbuild --target=x86_64-unknown-linux-musl
$ sudo docker run --rm -it -p 8080:8080 -v "$(pwd):/mnt" alpine:3.18
> apk add imagemagick ffmpeg exiftool
> cp /mnt/docker/prod/root/usr/lib/ImageMagick-7.1.1/config-Q16HDRI/policy.xml /usr/lib/ImageMagick-7.1.1/config-Q16HDRI/
> /mnt/target/x86_64-unknown-linux-musl/debug/pict-rs --log-targets debug run
```


## Contributing
Feel free to open issues for anything you find an issue with. Please note that any contributed code will be licensed under the AGPLv3.


## FAQ
### Question: Is pict-rs stateless
Answer: No. pict-rs relies on an embedded key-value store called `sled` to store metadata about
uploaded media. This database maintains a set of files on the local disk and cannot be configured to
use a network.

### Question: Can I use a different database with pict-rs
Answer: No. Currently pict-rs only supports the embedded key-value store called `sled`. In the
future, I would like to support `Postgres` and `BonsaiDB`, but I am currently not offering a
timeline on support. If you care about this and are a rust developer, I would accept changes.

### Question: How can I submit changes
Answer: If you would like to contribute to pict-rs, you can push your code to a public git host of
your choice and let me know you did so via matrix or email. I can pull and merge your changes into
this repository from there.

Alternatively, you are welcome to email me a patch that I can apply.

I will not be creating additional accounts on my forgejo server, sorry not sorry.

### Question: I want to configure it with yaml instead of toml
Answer: That's not a question, but you can configure pict-rs with json, hjson, yaml, ini, or toml.
Writing configs in other formats is left as an exercise to the reader.

### Question: How do I donate to pict-rs
Answer: You don't. I get paid by having a job where I do other stuff. Don't give me money that I
don't need.

## Common Problems
In some cases, pict-rs might crash and be unable to start again. The most common reason for this is
the filesystem reached 100% and pict-rs could not write to the disk, but this could also happen if
pict-rs is killed at an unfortunate time. If this occurs, the solution is to first get more disk for
your server, and then look in the `sled-repo` directory for pict-rs. It's likely that pict-rs
created a zero-sized file called `snap.somehash.generating`. Delete that file and restart pict-rs.

When running with the provided docker container, pict-rs might fail to start with an IO error saying
"permission denied". This problably means pict-rs' volume is not owned by the correct user. Changing
the ownership on the pict-rs volume to `991:991` should solve this problem.

## License

Copyright © 2022 Riley Trautman

pict-rs is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

pict-rs is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details. This file is part of pict-rs.

You should have received a copy of the GNU General Public License along with pict-rs. If not, see [http://www.gnu.org/licenses/](http://www.gnu.org/licenses/).
