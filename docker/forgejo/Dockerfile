FROM alpine:3.19

ARG UID=991
ARG GID=991

ENV \
  UID=${UID} \
  GID=${GID}

USER root
RUN \
  addgroup -g "${GID}" app && \
  adduser -D -G app -u "${UID}" -g "" -h /opt/app app && \
  apk add exiftool ffmpeg imagemagick imagemagick-heic tini && \
  chown -R app:app /mnt

COPY pict-rs /usr/local/bin/pict-rs

USER app
EXPOSE 6669
EXPOSE 8080
VOLUME /mnt
ENTRYPOINT ["/sbin/tini", "--"]
CMD ["/usr/local/bin/pict-rs", "run"]
