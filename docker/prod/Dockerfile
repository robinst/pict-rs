ARG REPO_ARCH=amd64

# cross-build environment
FROM asonix/$REPO_ARCH-latest AS builder

ARG TAG=main
ARG BINARY=pict-rs
ARG PROJECT=pict-rs
ARG GIT_REPOSITORY=https://git.asonix.dog/asonix/$PROJECT

ENV\
 BINARY=${BINARY}

ADD \
 --chown=build:build \
 $GIT_REPOSITORY/archive/$TAG.tar.gz \
 /opt/build/repo.tar.gz

RUN \
 tar zxf repo.tar.gz

WORKDIR /opt/build/$PROJECT

RUN \
 build

# production environment
FROM asonix/rust-runner:$REPO_ARCH-latest

USER root

COPY --from=builder \
 /opt/build/binary \
 /usr/local/bin/application

RUN \
 apk add ffmpeg imagemagick exiftool

RUN \
 chown -R app:app /mnt

COPY root/ /

VOLUME /mnt
USER app
EXPOSE 8080
ENTRYPOINT ["/sbin/tini", "--"]
CMD ["/usr/local/bin/application", "-p", "/mnt"]
