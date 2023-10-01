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
            4. [Nix](#nix)
    2. [Api](#api)
3. [Administration](#administration)
    1. [Backups](#backups)
    2. [0.4 to 0.5 Migration Guide](#0-4-to-0-5-migration-guide)
        1. [Overview](#overview)
        2. [Configuration Updates](#configuration-updates)
            1. [Image Changes](#image-changes)
            2. [Animation Changes](#animation-changes)
            3. [Video Changes](#video-changes)
        3. [Upgrading Directly to Postgres](#upgrading-directly-to-postgres)
    3. [Filesystem to Object Storage Migration](#filesystem-to-object-storage-migration)
        1. [Troubleshooting](#migration-troubleshooting)
    4. [Sled to Postgres Migration](#sled-to-postgres-migration)
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
or 0.4.x stable). If it is older, consider using an alternative option for installing pict-rs. I am
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

A notable issue here is imagemagick 7, which is not packaged in Debian Sid and therefore unavailable
in any version of Debian or Ubuntu. If you are running an ubuntu or debian system, consider using
the [Nix](#nix) installation and run method.

More information is available in the [Ubuntu and Debian docs](./docs/ubuntu-and-debian.md)

##### Compile from Source
pict-rs can be compiled from source using a recent version of the rust compiler. I do development
and produce releases on 1.72. pict-rs also requires the `protoc` protobuf compiler to be present at
build-time in order to enable use of [`tokio-console`](https://github.com/tokio-rs/console).

Like the Binary Download option, `imagemagick`, `ffmpeg`, and `exiftool` must be installed for
pict-rs to run properly.

##### Nix
pict-rs comes with an associated nix flake. This is useful for the development environment, but can
also be used to run a "production" version of pict-rs with all the neccessary dependencies already
provided.

The Nix package manager can be installed with [these instructions](https://nixos.org/download.html).
After installation, two experimental features must be enabled: `flake` and `nix-command`. These need
to be added in `/etc/nix/nix.conf`:
```
experimental-features = nix-command flakes
```

After enabling flakes, you can run `nix build` from the pict-rs source directory. This will produce
a nix package containing pict-rs and its dependencies. It will also create a `result` symlink in the
pict-rs directory that links to the newly built package. The contents of `result` should be a single
folder `bin` with a single file `pict-rs` inside. This file is a shell script that invokes the
`pict-rs` binary with the required `$PATH` to find imagemagick 7, ffmpeg 6, and exiftool. You can
treat this shell script as if it were the true pict-rs binary, passing it the same arguments you
would pict-rs.

Example:
```
./result/bin/pict-rs -c dev.toml run
```


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
- `GET /image/original/{alias}` Get a full-resolution image. `alias` here is the `file` key from the
    `/image` endpoint's JSON
- `GET /image/original?alias={alias}` Get a full-resolution image. `alias` here is the `file` key from
    the `/image` endpoint's JSON
    Available source arguments are
    - `?alias={alias}` Serve an original file by its alias
    - `?proxy={url}` This `proxy` field can be used to proxy external URLs through the original
        endpoint. These proxied images are removed from pict-rs some time after their last access.
        This time is configurable with `PICTRS__MEDIA__RETENTION__PROXY`. See
        (./pict-rs.toml)[./pict-rs.toml] for more information.
- `HEAD /image/original/{alias}` Returns just the headers from the analogous `GET` request.
- `HEAD /image/original?alias={alias}` Returns just the headers from the analogous `GET` request.
    Available source arguments are
    - `?alias={alias}` Serve an original file by its alias
    - `?proxy={url}` This `proxy` field can be used to proxy external URLs through the original
        endpoint. These proxied images are removed from pict-rs some time after their last access.
        This time is configurable with `PICTRS__MEDIA__RETENTION__PROXY`. See
        (./pict-rs.toml)[./pict-rs.toml] for more information.
- `GET /image/details/original/{alias}` for getting the details of a full-resolution image.
    The returned JSON is structured like so:
    ```json
    {
        "width": 800,
        "height": 537,
        "content_type": "image/webp",
        "created_at": "2022-04-08T18:33:42.957791698Z"
    }
    ```
- `GET /image/details/original?alias={alias}` Same as the above endpoint but with a query instead of
    a path

    Available source arguments are
    - `?alias={alias}` Serve an original file by its alias
    - `?proxy={url}` This `proxy` field can be used to get details about proxied images in pict-rs.
        These proxied images are removed from pict-rs some time after their last access. This time
        is configurable with `PICTRS__MEDIA__RETENTION__PROXY`. See (./pict-rs.toml)[./pict-rs.toml]
        for more information.
- `GET /image/process.{ext}?src={alias}&...` Get a file with transformations applied.
    Available source arguments are
    - `?src={alias}` This behavior is the same as in previous releases
    - `?alias={alias}` This `alias` field is the same as the `src` field. Renamed for better
        consistency
    - `?proxy={url}` This `proxy` field can be used to proxy external URLs through the process
        endpoint. These proxied images are removed from pict-rs some time after their last access.
        This time is configurable with `PICTRS__MEDIA__RETENTION__PROXY`. See
        (./pict-rs.toml)[./pict-rs.toml] for more information.
    
    Existing transformations include
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

    Supported `ext` file extensions include `apng`, `avif`, `gif`, `jpg`, `jxl`, `png`, and `webp`.
    Note that while `avif` and `webp` will work for both animated & non-animated images, some
    formats like `apng` and `gif` are only used to serve animations while others like `jpg`, `jxl`
    and `png` are only used to serve sill images.

    An example of usage could be
    ```
    GET /image/process.jpg?src=asdf.png&thumbnail=256&blur=3.0
    ```
    which would create a 256x256px JPEG thumbnail and blur it
- `HEAD /image/process.{ext}?src={alias}` Returns just the headers from the analogous `GET` request.
    Returns 404 if the processed image has not been generated yet.

    Available source arguments are
    - `?src={alias}` This behavior is the same as in previous releases
    - `?alias={alias}` This `alias` field is the same as the `src` field. Renamed for better
        consistency
    - `?proxy={url}` This `proxy` field can be used to get headers for proxied images.
- `GET /image/process_backgrounded.{ext}?src={alias}&...` queue transformations to be applied to a
    given file. This accepts the same arguments as the `process.{ext}` endpoint, but does not wait
    for the processing to complete.

    Available source arguments are
    - `?src={alias}` This behavior is the same as in previous releases
    - `?alias={alias}` This `alias` field is the same as the `src` field. Renamed for better
        consistency
- `GET /image/details/process.{ext}?src={alias}&...` for getting the details of a processed image.
    The returned JSON is the same format as listed for the full-resolution details endpoint.

    Available source arguments are
    - `?src={alias}` This behavior is the same as in previous releases
    - `?alias={alias}` This `alias` field is the same as the `src` field. Renamed for better
        consistency
    - `?proxy={url}` This `proxy` field can be used to get details about proxied images.
- `GET /image/details/process.{ext}?alias={alias}` Same as the above endpoint but with a query
    instead of a path

    Available source arguments are
    - `?alias={alias}` Serve a processed file by its alias
    - `?proxy={url}` This `proxy` field can be used to get details about proxied images in pict-rs.
        These proxied images are removed from pict-rs some time after their last access. This time
        is configurable with `PICTRS__MEDIA__RETENTION__PROXY`. See [./pict-rs.toml](./pict-rs.toml)
        for more information.
- `DELETE /image/delete/{delete_token}/{alias}` or `GET /image/delete/{delete_token}/{alias}` to
    delete a file, where `delete_token` and `alias` are from the `/image` endpoint's JSON
- `GET /healthz` Check the health of the pict-rs server. This will check that the `sled` embedded
    database is functional and that the configured store is accessible


The following endpoints are protected by an API key via the `X-Api-Token` header, and are disabled
unless the `--api-key` option is passed to the binary or the PICTRS__SERVER__API_KEY environment variable is
set.

A secure API key can be generated by any password generator.
- `POST /internal/import` for uploading an image while preserving the filename as the first alias.
    The upload format and response format are the same as the `POST /image` endpoint.
- `POST /internal/delete?alias={alias}` Delete an alias without requiring a delete token.
    Available source arguments are
    - `?alias={alias}` Delete a file alias
    - `?proxy={url}` Delete a proxied file's alias

    This endpoint returns the following JSON
    ```json
    {
        "msg": "ok",
    }
    ```
- `POST /internal/purge?alias={alias}` Purge a file by it's alias. This removes all aliases and
    files associated with the query.

    Available source arguments are
    - `?alias={alias}` Purge a file by it's alias
    - `?proxy={url}` Purge a proxied file by its URL

    This endpoint returns the following JSON
    ```json
    {
        "msg": "ok",
        "aliases": ["asdf.png"]
    }
    ```
- `GET /internal/aliases?alias={alias}` Get the aliases for a file by its alias

    Available source arguments are
    - `?alias={alias}` Get all aliases for a file by the provided alias
    - `?proxy={url}` Get all aliases for a proxied file by its url

    This endpiont returns the same JSON as the purge endpoint
- `DELETE /internal/variants` Queue a cleanup for generated variants of uploaded images.

    If any of the cleaned variants are fetched again, they will be re-generated.
- `GET /internal/identifier?alias={alias}` Get the image identifier (file path or object path) for a
    given alias.

    Available source arguments are
    - `?alias={alias}` Get the identifier for a file by the provided alias
    - `?proxy={url}` Get the identifier for a proxied file by its url

    On success, the returned json should look like this:
    ```json
    {
        "msg": "ok",
        "identifier": "/path/to/object"
    }
    ```
- `POST /internal/set_not_found?alias={alias}` Set the 404 image that is served from the original
    and process endpoints. The image used must already be uploaded and have an alias. The request
    should look like this:
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

    In the event pict-rs can't find the provided alias, it will return a 400 Bad Request with the
    following json:
    ```json
    {
        "msg": "No hash associated with provided alias"
    }
    ```
- `POST /internal/export` Export the current sled database to the configured `export_path`. This is
    useful for taking backups of a running pict-rs server. On success, it will return
    ```json
    {
        "msg": "ok"
    }
    ```

    Restoring from an exported database is as simple as:
    1. Stopping pict-rs
    2. Moving your current `sled-repo` directory to a safe location (e.g. `sled-repo.bak`)
        ```bash
        $ mv sled-repo sled-repo.bak
        ```
    3. Copying an exported database to `sled-repo`
        ```bash
        $ cp -r exports/2023-07-08T22:26:21.194126713Z sled-repo
        ```
    4. Starting pict-rs
- `GET /internal/hashes?{query}` Get a page of hashes ordered by newest to oldest based on the
    provided query. On success, it will return the following json:
    ```json
    {
        "msg": "ok",
        "page": {
            "limit": 20,
            "current": "some-long-slug-string",
            "next": "some-long-slug-string",
            "prev": "some-long-slug-string",
            "hashes": [{
                "hex": "some-long-hex-encoded-hash",
                "aliases": [
                    "file-alias.png",
                    "another-alias.png",
                ],
                "details": {
                    "width": 1920,
                    "height": 1080,
                    "frames": 30,
                    "content_type": "video/mp4",
                    "created_at": "2022-04-08T18:33:42.957791698Z"
                }
            }]
        }
    }
    ```
    Note that some fields in this response are optional (including `next`, `prev`, `current`, `details` and `frames`)

    Available query options:
    - empty: this fetches the first page of the results (e.g. the newest media)
    - `?slug={slug}` this fetches a specific page of results. the `slug` field comes from the
        `current`, `next`, or `prev` fields in the page json
    - `?timestamp={timestamp}` this fetches results older than the specified timestamp for easily
        searching into the data. the `timestamp` should be formatted according to RFC3339
    - `?limit={limit}` specifies how many results to return per page


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

Finally, there's an optional prometheus scrape endpoint that can be enabled with the
`PICTRS__METRICS__PROMETHEUS_ADDRESS` configuration. This binds to a separate port from the main
pict-rs application. See [pict-rs.toml](./pict-rs.toml) for more details.


## Administration
### Backups
pict-rs maintains two folders that matter: the `sled-repo` directory, and the `files` directory.
`sled-repo` is where it keeps metadata about files such as: their location, their aliases, their
processed versions' locations, their dimensions, mime type, etc. `files` is where it puts uploaded
files when storing on the local filesystem.

The `sled-repo` folder is generally small compared to the `files` folder, and backing it up can be
as simple as copying the folder somewhere else. I recommend doing so while pict-rs is not running.

If you can't stop pict-rs, but would like to back up the database, there is an internal endpoint at
`/internal/export` documented in [Api](#api) that can be used to produce a copy of the current
database for easy backups.

### 0.4 to 0.5 Migration Guide
#### Overview
pict-rs will automatically migrate from the 0.4 db format to the 0.5 db format on the first launch
of 0.5. This process might take a while, especially if you've been running pict-rs since before 0.3.
The reason for this is pict-rs now requires original files to have associated details records stored
in the database, and while generating these records happened by default for 0.3 and 0.4, images
uploaded before this was standard may not have ever had their details records generated.

_This upgrade must be performed while pict-rs is offline._

#### Configuration Updates
Previously, pict-rs only had two categories for files: images and videos. pict-rs 0.5 adds a third
category: animation. With the new explicit support for animated file types, some configuration
options have moved.

##### Image Changes
| Old Environment Variable       | New Environment Variable              |
| ------------------------------ | ------------------------------------- |
| `PICTRS__MEDIA__FORMAT`        | `PICTRS__MEDIA__IMAGE__FORMAT`        |
| `PICTRS__MEDIA__MAX_WIDTH`     | `PICTRS__MEDIA__IMAGE__MAX_WIDTH`     |
| `PICTRS__MEDIA__MAX_HEIGHT`    | `PICTRS__MEDIA__IMAGE__MAX_HEIGHT`    |
| `PICTRS__MEDIA__MAX_AREA`      | `PICTRS__MEDIA__IMAGE__MAX_AREA`      |
|                                | `PICTRS__MEDIA__IMAGE__MAX_FILE_SIZE` |

| Old TOML Value          | New TOML Value                |
| ----------------------- | ----------------------------- |
| `[media] format`        | `[media.image] format`        |
| `[media] max_width`     | `[media.image] max_width`     |
| `[media] max_height`    | `[media.image] max_height`    |
| `[media] max_area`      | `[media.image] max_area`      |
|                         | `[media.image] max_file_size` |

##### Animation Changes
| Old Environment Variable              | New Environment Variable                    |
| ------------------------------------- | ------------------------------------------- |
| `PICTRS__MEDIA__GIF__MAX_WIDTH`       | `PICTRS__MEDIA__ANIMATION__MAX_WIDTH`       |
| `PICTRS__MEDIA__GIF__MAX_HEIGHT`      | `PICTRS__MEDIA__ANIMATION__MAX_HEIGHT`      |
| `PICTRS__MEDIA__GIF__MAX_AREA`        | `PICTRS__MEDIA__ANIMATION__MAX_AREA`        |
| `PICTRS__MEDIA__GIF__MAX_FILE_SIZE`   | `PICTRS__MEDIA__ANIMATION__MAX_FILE_SIZE`   |
| `PICTRS__MEDIA__GIF__MAX_FRAME_COUNT` | `PICTRS__MEDIA__ANIMATION__MAX_FRAME_COUNT` |
|                                       | `PICTRS__MEDIA__ANIMATION__FORMAT`          |
|                                       | `PICTRS__MEDIA__ANIMATION__MAX_FILE_SIZE`   |

| Old TOML Value                | New TOML Value                      |
| ----------------------------- | ----------------------------------- |
| `[media.gif] max_width`       | `[media.animation] max_width`       |
| `[media.gif] max_height`      | `[media.animation] max_height`      |
| `[media.gif] max_area`        | `[media.animation] max_area`        |
| `[media.gif] max_file_size`   | `[media.animation] max_file_size`   |
| `[media.gif] max_frame_count` | `[media.animation] max_frame_count` |
|                               | `[media.animation] format`          |
|                               | `[media.animation] max_file_size`   |

##### Video Changes
| Old Environment Variable             | New Environment Variable                |
| ------------------------------------ | --------------------------------------- |
| `PICTRS__MEDIA__ENABLE_SILENT_VIDEO` | `PICTRS__MEDIA__VIDEO__ENABLE`          |
| `PICTRS__MEDIA__ENABLE_FULL_VIDEO`   | `PICTRS__MEDIA__VIDEO__ALLOW_AUDIO`     |
| `PICTRS__MEDIA__VIDEO_CODEC`         | `PICTRS__MEDIA__VIDEO__VIDEO_CODEC`     |
| `PICTRS__MEDIA__AUDIO_CODEC`         | `PICTRS__MEDIA__VIDEO__AUDIO_CODEC`     |
| `PICTRS__MEDIA__MAX_FRAME_COUNT`     | `PICTRS__MEDIA__VIDEO__MAX_FRAME_COUNT` |
| `PICTRS__MEDIA__ENABLE_FULL_VIDEO`   | `PICTRS__MEDIA__VIDEO__ALLOW_AUDIO`     |
|                                      | `PICTRS__MEDIA__VIDEO__MAX_WIDTH`       |
|                                      | `PICTRS__MEDIA__VIDEO__MAX_HEIGHT`      |
|                                      | `PICTRS__MEDIA__VIDEO__MAX_AREA`        |
|                                      | `PICTRS__MEDIA__VIDEO__MAX_FILE_SIZE`   |

| Old TOML Value                | New TOML Value                  |
| ----------------------------- | ------------------------------- |
| `[media] enable_silent_video` | `[media.video] enable`          |
| `[media] enable_full_video`   | `[media.video] allow_audio`     |
| `[media] video_codec`         | `[media.video] video_codec`     |
| `[media] audio_codec`         | `[media.video] audio_codec`     |
| `[media] max_frame_count`     | `[media.video] max_frame_count` |
| `[media] enable_full_video`   | `[media.video] allow_audio`     |
|                               | `[media.video] max_width`       |
|                               | `[media.video] max_height`      |
|                               | `[media.video] max_area`        |
|                               | `[media.video] max_file_size`   |

Note that although each media type now includes its own `MAX_FILE_SIZE` configuration, the
`PICTRS__MEDIA__MAX_FILE_SIZE` value still exists as a global limit for any file type.

In addition to all the configuration options mentioned above, there are now individual quality
settings that can be configured for each image and animation type, as well as for video files.
Please see the [pict-rs.toml](./pict-rs.toml) file for more information.

#### Upgrading Directly to Postgres
pict-rs supports migrating directly to the postgres repo during the upgrade. In order to do this,
the postgres repo needs to be configured and the `old_repo` needs to be specified. The `old_repo`
section just contains the `path` of the `repo` section in your 0.4 config.

Example:
```toml
[old_repo]
path = '/mnt/sled-repo'

[repo]
type = 'postgres'
url = 'postgres://user:password@host:5432/db'
```

Or with environment varaibles:
```
PICTRS__OLD_REPO__PATH=/mnt/sled-repo
PICTRS__REPO__TYPE=postgres
PICTRS__REPO__URL=postgres://user:password@host:5432/db
```

Once these variables are set, 0.5 can be started and the migration will automatically occur.

### Filesystem to Object Storage migration
_Make sure you take a backup of the sled-repo directory before running this command!!! Migrating to
object storage updates the database and if you need to revert for any reason, you'll want a backup._

It is possible to migrate to object storage. This can be useful if
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
> pict-rs --version # verify pict-rs version is recent (should probably be 0.4.0 or later)
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

### Sled to Postgres Migration
If you upgraded to 0.5 without migrating to postgres at the same time, you can migrate to postgres
after the fact with the built-in migration utility. Before running the migration, make sure you have
a postgres role and database ready for pict-rs. The first thing pict-rs will do upon connecting to
a new database is attempt to add the `pgcrypto` extension, so if the role you created for pict-rs
does not have that permission, it will fail.

The migration command is fairly simple. It just needs the path to your existing repo and the URL to
your new repo.

```bash
$ pict-rs \
    migrate-repo \
    sled -p /path/to-/sled-repo \
    postgres -u postgres://user:password@host:5432/db
```

If you're running with docker-compose, this could look like the following:
```bash
$ sudo docker compose stop pictrs # stop the pict-rs container
$ sudo docker compose run pictrs sh # launch a shell in the pict-rs container
> pict-rs --version # verify pict-rs version is recent (should probably be 0.5.0 or later)
> pict-rs \
    migrate-repo \
    sled -p /mnt/sled-repo \
    postgres -u postgres://user:password@host:5432/db
> exit
$ vi docker-compose.yml # edit the docker-compose yaml however you like to edit it, make sure all the variables described below are set
$ sudo docker compose up -d pictrs # start pict-rs again after the migration. Note that this is not 'docker compose start'. using the `up` subcommand explicitly reloads configurations
```

_This command must be run while pict-rs is offline._

This migration should be pretty quick, since there's no actual files getting moved around. After the
migration completes, make sure pict-rs is configured to use the postgres repo, then start it back
up.

Example:
```toml
[repo]
type = 'postgres'
url = 'postgres://user:password@host:5432/db'
```
or
```
PICTRS__REPO__TYPE=postgres
PICTRS__REPO__URL=postgres://user:password@host:5432/db
```


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
