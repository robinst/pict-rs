#!/usr/bin/env bash

set -xe

certstrap init --common-name pictrsCA
certstrap request-cert --common-name postgres --domain localhost
certstrap sign postgres --CA pictrsCA

mkdir -p ./storage/
sudo mkdir -p ./storage/postgres

sudo tee ./storage/postgres/pg_hba.conf << EOF
host all all all trust
hostssl all all all cert clientcert=verify-full
EOF
