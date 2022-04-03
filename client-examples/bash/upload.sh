#!/usr/bin/env bash

set -xe

upload_ids=$(
  curl \
    -F "images[]=@../cat.jpg" \
    -F "images[]=@../earth.gif" \
    -F "images[]=@../scene.webp" \
    -F "images[]=@../test.png" \
    -F "images[]=@../earth.gif" \
    -F "images[]=@../test.png" \
    -F "images[]=@../cat.jpg" \
    -F "images[]=@../scene.webp" \
    'http://localhost:8080/image/backgrounded' | \
  jq '.uploads[].upload_id' | \
  sed 's/"//g'
)

for upload in $(echo $upload_ids)
do
  echo "Processing for $upload"

  json=$(curl "http://localhost:8080/image/backgrounded/claim?upload_id=$upload") 
  delete_token=$(echo $json | jq '.files[0].delete_token' | sed 's/"//g')
  filename=$(echo $json | jq '.files[0].file' | sed 's/"//g')

  details=$(curl "http://localhost:8080/image/details/original/$filename")
  mime_type=$(echo $details | jq '.content_type' | sed 's/"//g')

  echo "Original mime: $mime_type"

  curl "http://localhost:8080/image/process_backgrounded.webp?src=$filename&resize=200"
  sleep 1
  details=$(curl "http://localhost:8080/image/details/process.webp?src=$filename&resize=200")
  mime_type=$(echo $details | jq '.content_type' | sed 's/"//g')

  echo "Processed mime: $mime_type"

  curl "http://localhost:8080/image/delete/$delete_token/$filename"
done
