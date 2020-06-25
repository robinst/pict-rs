#!/usr/bin/env bash

ARCH=${1:-amd64}

export USER_ID=$(id -u)
export GROUP_ID=$(id -g)

docker-compose build --pull
docker-compose run --service-ports pictrs-$ARCH
