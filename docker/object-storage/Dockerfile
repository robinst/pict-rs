FROM archlinux:latest

ARG UID=1000
ARG GID=1000

RUN \
 pacman -Syu --noconfirm \
        perl-image-exiftool \
        imagemagick \
        ffmpeg && \
 groupadd -g 1000 app && \
 useradd -m \
        -d /opt/app \
        -u $UID \
        -g $GID \
        app

COPY ./pict-rs.toml /etc/pict-rs.toml

ENV PATH=$PATH:/usr/bin/vendor_perl

WORKDIR /opt/app
USER app
