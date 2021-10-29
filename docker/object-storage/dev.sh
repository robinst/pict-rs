#!/usr/bin/env bash

ARCH=${1:-amd64}

export USER_ID=$(id -u)
export GROUP_ID=$(id -g)

sudo docker-compose build --pull
sudo docker-compose up -d minio
sudo docker-compose up -d pictrs_proxy
sudo docker-compose run --service-ports --use-aliases pictrs
sudo docker-compose down
