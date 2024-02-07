FROM docker.io/asonix/rust-builder:latest-linux-amd64

RUN rustup component add clippy

COPY entrypoint.sh /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
