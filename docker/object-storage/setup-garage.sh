#!/usr/bin/env bash

STDOUT=./out.log
STDERR=./err.log

touch "${STDOUT}"
touch "${STDERR}"
rm "${STDOUT}" "${STDERR}"

KEY_ID='GK2182acf19c2bdb8b9c20e16e'
SECRET_KEY='0072105b8659adc02cce21d9135a88ebc279b3a35e170d23d31c63fb9307a168'

node_id=$(garage -c garage-local.toml status 2>>"${STDERR}" | tail -n 1 | awk '{ print $1 }')

garage -c garage-local.toml layout assign -z dc1 -c 50GB "${node_id}" >>"${STDOUT}" 2>>"${STDERR}"
garage -c garage-local.toml layout apply --version 1 >>"${STDOUT}" 2>>"${STDERR}"

garage -c garage-local.toml bucket create pict-rs >>"${STDOUT}" 2>>"${STDERR}"

garage -c garage-local.toml key import \
  -n pict-rs-key --yes "${KEY_ID}" "${SECRET_KEY}" \
  >> "${STDOUT}" 2>> "${STDERR}"

garage -c garage-local.toml bucket allow --read --write --owner pict-rs --key pict-rs-key >>"${STDOUT}" 2>>"${STDERR}"
garage -c garage-local.toml bucket website pict-rs --allow >> "${STDOUT}" 2>>"${STDERR}"

cat > pict-rs-garage.toml <<EOF
[server]
address = '0.0.0.0:8080'
api_key = 'api-key'
max_file_count = 10

[tracing.logging]
format = 'normal'
targets = 'info'

[tracing.console]
buffer_capacity = 102400

[tracing.opentelemetry]
url = 'http://jaeger:4317'
service_name = 'pict-rs'
targets = 'info,pict_rs=debug'

[metrics]
prometheus_address = "127.0.0.1:8070"

[media]
max_file_size = 40
filters = ['blur', 'crop', 'identity', 'resize', 'thumbnail']

[media.image]
format = "webp"

[media.image.quality]
avif = 50
png = 50
jpeg = 75
jxl = 50
webp = 60

[media.animation]
max_width = 2000
max_height = 2000
max_area = 2000000
format = "webp"

[media.animation.quality]
apng = 50
avif = 50
webp = 60

[media.video]
enable = true
allow_audio = true
max_area = 884000000
video_codec = "vp9"

[media.video.quality]
crf_240 = 37
crf_360 = 36
crf_480 = 33
crf_720 = 32
crf_1080 = 31
crf_1440 = 24
crf_2160 = 15
crf_max = 12

[repo]
type = 'postgres'
url = 'postgres://pictrs:1234@localhost:5432/pictrs'

[store]
type = 'object_storage'
endpoint = 'http://garage:3900'
use_path_style = true
bucket_name = 'pict-rs'
region = 'garage'
access_key = '${KEY_ID}'
secret_key = '${SECRET_KEY}'
EOF
