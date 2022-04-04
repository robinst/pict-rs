ARG REPO_ARCH

FROM asonix/rust-runner:latest-linux-$REPO_ARCH

USER root
RUN \
  apk add exiftool imagemagick ffmpeg && \
  chown -R app:app /mnt

COPY pict-rs /usr/local/bin/pict-rs
COPY docker/prod/root/ /

USER app
EXPOSE 6669
EXPOSE 8080
VOLUME /mnt
ENTRYPOINT ["/sbin/tini", "--"]
CMD ["/usr/local/bin/pict-rs", "run"]
