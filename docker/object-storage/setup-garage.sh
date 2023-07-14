#!/usr/bin/env bash

STDOUT=./out.log
STDERR=./err.log

touch "${STDOUT}"
touch "${STDERR}"
rm "${STDOUT}" "${STDERR}"

node_id=$(garage -c garage-local.toml status 2>>"${STDERR}" | tail -n 1 | awk '{ print $1 }')

garage -c garage-local.toml layout assign -z dc1 -c 1 "${node_id}" >>"${STDOUT}" 2>>"${STDERR}"
garage -c garage-local.toml layout apply --version 1 >>"${STDOUT}" 2>>"${STDERR}"

garage -c garage-local.toml bucket create pict-rs >>"${STDOUT}" 2>>"${STDERR}"

garage -c garage-local.toml key new --name pict-rs-key >>"${STDOUT}" 2>>"${STDERR}"
key_id=$(garage -c garage-local.toml key info pict-rs-key 2>>"${STDERR}" | grep "Key ID" | awk '{ print $3 }')
secret_key=$(garage -c garage-local.toml key info pict-rs-key 2>>"${STDERR}" | grep "Secret key" | awk '{ print $3 }')

garage -c garage-local.toml bucket allow --read --write --owner pict-rs --key pict-rs-key >>"${STDOUT}" 2>>"${STDERR}"
garage -c garage-local.toml bucket website pict-rs --allow >> "${STDOUT}" 2>>"${STDERR}"

cat > pict-rs-garage.toml <<EOF
[server]
address = '0.0.0.0:8080'
worker_id = 'pict-rs-1'

[tracing.logging]
format = 'normal'
targets = 'warn,tracing_actix_web=info,actix_server=info,actix_web=info'

[tracing.console]
buffer_capacity = 102400

[tracing.opentelemetry]
service_name = 'pict-rs'
targets = 'info'

[old_db]
path = '/mnt'

[media]
max_width = 10000
max_height = 10000
max_area = 40000000
max_file_size = 40
enable_silent_video = true
enable_full_video = true
video_codec = "vp9"
filters = ['blur', 'crop', 'identity', 'resize', 'thumbnail']
skip_validate_imports = false

[repo]
type = 'sled'
path = '/mnt/sled-repo-garage'
cache_capacity = 67108864

[store]
type = 'object_storage'
endpoint = 'http://garage:3900'
use_path_style = true
bucket_name = 'pict-rs'
region = 'garage'
access_key = '${key_id}'
secret_key = '${secret_key}'
EOF
