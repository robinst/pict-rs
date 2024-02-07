FROM docker.io/asonix/rust-builder:latest-linux-amd64

RUN rustup component add clippy

COPY .forgejo/actions/build-amd64/entrypoint.sh /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
