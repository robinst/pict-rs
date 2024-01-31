#!/usr/bin/env bash

set -xe

mkdir -p ./data

certstrap --depot-path ./data init --common-name pictrsCA
certstrap --depot-path ./data request-cert --common-name pictrs --domain localhost
certstrap --depot-path ./data sign pictrs --CA pictrsCA
