# pict-rs
_a simple image hosting service_

## Links
- Find the code on [gitea](https://git.asonix.dog/asonix/pict-rs)
- Join the discussion on [matrix](https://matrix.to/#/#pictrs:matrix.asonix.dog?via=matrix.asonix.dog)
- Hit me up on [mastodon](https://masto.asonix.dog/@asonix)

## Usage
### Running
```
pict-rs 0.4.0-alpha.1
asonix <asonix@asonix.dog>
A simple image hosting service

USAGE:
    pict-rs [OPTIONS] <SUBCOMMAND>

OPTIONS:
    -c, --config-file <CONFIG_FILE>
            Path to the pict-rs configuration file

        --console-address <CONSOLE_ADDRESS>
            Address and port to expose tokio-console metrics

        --console-buffer-capacity <CONSOLE_BUFFER_CAPACITY>
            Capacity of the console-subscriber Event Buffer

    -h, --help
            Print help information

        --log-format <LOG_FORMAT>
            Format of logs printed to stdout

        --log-targets <LOG_TARGETS>
            Log levels to print to stdout, respects RUST_LOG formatting

        --old-db-path <OLD_DB_PATH>
            Path to the old pict-rs sled database

        --opentelemetry-service-name <OPENTELEMETRY_SERVICE_NAME>
            Service Name to use for OpenTelemetry

        --opentelemetry-targets <OPENTELEMETRY_TARGETS>
            Log levels to use for OpenTelemetry, respects RUST_LOG formatting

        --opentelemetry-url <OPENTELEMETRY_URL>
            URL to send OpenTelemetry metrics

        --save-to <SAVE_TO>
            File to save the current configuration for reproducible runs

    -V, --version
            Print version information

SUBCOMMANDS:
    filesystem        Migrate from the provided filesystem storage
    help              Print this message or the help of the given subcommand(s)
    object-storage    Migrate from the provided object storage
    run               Runs the pict-rs web server
```

```
pict-rs-run 
Runs the pict-rs web server

USAGE:
    pict-rs run [OPTIONS] [SUBCOMMAND]

OPTIONS:
    -a, --address <ADDRESS>
            The address and port to bind the pict-rs web server

        --api-key <API_KEY>
            The API KEY required to access restricted routes

    -h, --help
            Print help information

        --media-enable-silent-video <MEDIA_ENABLE_SILENT_VIDEO>
            Whether to enable GIF and silent MP4 uploads. Full videos are unsupported

        --media-filters <MEDIA_FILTERS>
            Which media filters should be enabled on the `process` endpoint

        --media-format <MEDIA_FORMAT>
            Enforce uploaded media is transcoded to the provided format

        --media-max-area <MEDIA_MAX_AREA>
            The maximum area, in pixels, for uploaded media

        --media-max-file-size <MEDIA_MAX_FILE_SIZE>
            The maximum size, in megabytes, for uploaded media

        --media-max-height <MEDIA_MAX_HEIGHT>
            The maximum height, in pixels, for uploaded media

        --media-max-width <MEDIA_MAX_WIDTH>
            The maximum width, in pixels, for uploaded media

        --media-skip-validate-imports <MEDIA_SKIP_VALIDATE_IMPORTS>
            Whether to validate media on the "import" endpoint

        --worker-id <WORKER_ID>
            

SUBCOMMANDS:
    filesystem        Run pict-rs with filesystem storage
    help              Print this message or the help of the given subcommand(s)
    object-storage    Run pict-rs with object storage
```

Try running `help` commands for more runtime configuration options
```
$ pict-rs run filesystem -h
$ pict-rs run object-storage -h
$ pict-rs run filesystem sled -h
$ pict-rs run object-storage sled -h
```

See [`pict-rs.toml`](https://git.asonix.dog/asonix/pict-rs/src/branch/main/pict-rs.toml) for more
configuration

#### Example:
Run with the default configuration
```
$ ./pict-rs run
```
Running on all interfaces, port 8080, storing data in /opt/data
```
$ ./pict-rs run -a 0.0.0.0:8080 filesystem -p /opt/data/files sled -p /opt/data/sled-repo
```
Running locally, port 9000, storing data in data/, and converting all uploads to PNG
```
$ ./pict-rs run -a 127.0.0.1:9000 --media-format png filesystem -p data/files sled -p data/sled-repo
```
Running locally, port 8080, storing data in data/, and only allowing the `thumbnail` and `identity` filters
```
$ ./pict-rs run -a 127.0.0.1:8080 --media-filters thumbnail --media-filters identity filesystem -p data/files sled -p data/sled-repo
```
Running from a configuration file
```
$ ./pict-rs -c ./pict-rs.toml run
```
Migrating to object storage from filesystem storage
```
$ ./pict-rs filesystem -p data/sled-repo object-storage -a ACCESS_KEY -b BUCKET_NAME -r REGION -s SECRET_KEY
```
Dumping configuration overrides to a toml file
```
$ ./pict-rs --save-to pict-rs.toml run object-storage -a ACCESS_KEY -b pict-rs -r us-east-1 -s SECRET_KEY sled -p data/sled-repo
```

#### Docker
Run the following commands:
```
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

#### Docker Development
###### With Arch
```
$ cargo build
$ sudo docker run --rm -it -p 8080:8080 -v "$(pwd):/mnt" archlinux:latest
# pacman -Syu imagemagick ffmepg perl-image-exiftool
# cp /mnt/docker/prod/root/usr/lib/ImageMagick-7.1.0/config-Q16HDRI/policy.xml /usr/lib/ImageMagick-7.1.0/config-Q16HDRI/
# PATH=$PATH:/usr/bin/vendor_perl RUST_LOG=debug /mnt/target/debug/pict-rs run
```
###### With Alpine
```
$ cross build --target=x86_64-unknown-linux-musl
$ sudo docker run --rm -it -p 8080:8080 -v "$(pwd):/mnt alpine:3.15
# apk add imagemagick ffmpeg exiftool
# cp /mnt/docker/prod/root/usr/lib/ImageMagick-7.1.0/config-Q16HDRI/policy.xml /usr/lib/ImageMagick-7.1.0/config-Q16HDRI/
# RUST_LOG=debug /mnt/target/x86_64-unknown-linux-musl/debug/pict-rs RUN
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
                    "created_at": [
                        2020,
                        345,
                        67376,
                        394363487
                    ]
                }
            },
            {
                "delete_token": "kAYy9nk2WK",
                "file": "8qFS0QooAn.jpg",
                "details": {
                    "width": 400,
                    "height": 400,
                    "content_type": "image/jpeg",
                    "created_at": [
                        2020,
                        345,
                        67376,
                        394363487
                    ]
                }
            },
            {
                "delete_token": "OxRpM3sf0Y",
                "file": "1hJaYfGE01.jpg",
                "details": {
                    "width": 400,
                    "height": 400,
                    "content_type": "image/jpeg",
                    "created_at": [
                        2020,
                        345,
                        67376,
                        394363487
                    ]
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
- `GET /image/download?url={url}&backgrounded=(true|false)&ephemeral=(true|false)` Download an image
    from a remote server, returning the same JSON payload as the `POST` endpoint by default.

    if `backgrounded` is set to `true`, then the ingest processing will be queued for later and the
    response json will be the same as the `/image/backgrounded` endpoint.

    if `ephemeral` is set to true, the downloaded image will be marked as a "cached" image, and
    automatically removed from pict-rs N hours after its last access. The duration is configurable
    with the `--media-cache-duration` run flag, or the `[media] cache_duration` toml option.
- `GET /image/backgrounded/claim?upload_id={uuid}` Wait for a backgrounded upload to complete, claiming it's result
    Possible results:
    - 200 Ok (validation and ingest complete):
        ```json
        {
            "files": [
                {
                    "delete_token": "OxRpM3sf0Y",
                    "file": "1hJaYfGE01.jpg"
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
        "created_at": [
            2020,
            345,
            67376,
            394363487
        ]
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
    - `crop={int-w}x{int-h}`: produce a cropped version of the image with an `{int-w}` by `{int-h}`
        aspect ratio. The resulting crop will be centered on the image. Either the width or height
        of the image will remain full-size, depending on the image's aspect ratio and the requested
        aspect ratio. For example, a 1600x900 image cropped with a 1x1 aspect ratio will become 900x900. A
        1600x1100 image cropped with a 16x9 aspect ratio will become 1600x900.

    Supported `ext` file extensions include `png`, `jpg`, and `webp`

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
- `POST /internal/purge?alias={alias} Purge a file by it's alias. This removes all aliases and
    files associated with the query.

    This endpoint returns the following JSON
    ```json
    {
        "msg": "ok",
        "aliases": ["asdf.png"]
    }
    ```
- `GET /internal/aliases?alias={alias} Get the aliases for a file by it's alias
    - `?alias={alias}` get aliases by alias

    This endpiont returns the same JSON as the purge endpoint

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


## Contributing
Feel free to open issues for anything you find an issue with. Please note that any contributed code will be licensed under the AGPLv3.

## FAQ
### Question: I want to configure it with yaml instead of toml
Answer: That's not a question, but you can configure pict-rs with json, hjson, yaml, ini, or toml.
Writing configs in other formats is left as an exercise to the reader.

## License

Copyright © 2022 Riley Trautman

pict-rs is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

pict-rs is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details. This file is part of pict-rs.

You should have received a copy of the GNU General Public License along with pict-rs. If not, see [http://www.gnu.org/licenses/](http://www.gnu.org/licenses/).
