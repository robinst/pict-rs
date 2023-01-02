FROM alpine:edge

ARG UID=1000
ARG GID=1000

RUN \
  apk add exiftool imagemagick ffmpeg && \
  addgroup -g $GID app && \
  adduser -h /opt/app -g "" -G app -u $UID -D app && \
  chown -R app:app /mnt

COPY root/ /

WORKDIR /opt/app
USER app
