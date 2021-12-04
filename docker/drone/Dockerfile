ARG REPO_ARCH

FROM asonix/rust-runner:$REPO_ARCH-latest

USER root
RUN \
  apk add exiftool imagemagick ffmpeg && \
  chown -R app:app /mnt

COPY pict-rs /usr/local/bin/pict-rs
COPY docker/prod/root/ /

USER app
EXPOSE 8080
VOLUME /mnt
ENTRYPOINT ["/sbin/tini", "--"]
CMD ["/usr/local/bin/pict-rs", "-p", "/mnt"]
