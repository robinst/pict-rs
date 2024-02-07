#!/usr/bin/env bash

set -e

name=$1
action=$2
build_mode=$3

cargo "${action}" "--${build_mode}" "--target=${TARGET}"

case "$action" in
  build)
    echo "path=target/${TARGET}/release/${name}" >> "${GITHUB_OUTPUT}"
    ;;
  check)
    ;;
  test)
    ;;
  clippy)
    ;;
esac
