# pict-rs on Ubuntu and Debian

### The problem

At the time of writing, ImageMagick 7 has not been packaged for Debian Sid. This is a problem for
pict-rs, which depends on ImageMagick 7's commandline interface for media processing. Ubuntu users
are also affected, since Ubuntu inherits the Imagemagick package from Debian in the `universe`
archive.

pict-rs is also developed against ffmpeg 6, although from my testing it seems like the required
interfaces exist as far back as ffmpeg 4.4, which is the current stable version in Ubuntu 22.04. I
believe ffmpeg 5 is being prepped for the next Ubuntu release (23.10).

### Possible Solutions

Running pict-rs on an Ubuntu or Debian system can be done in the following ways:
1. Download the [ImageMagick AppImage](https://imagemagick.org/script/download.php). This is option
    only works for running pict-rs on x86_64
2. Compile ImageMagick 7 from source. User MichelSup in the [pict-rs matrix
    channel](https://matrix.to/#/%23pictrs:matrix.asonix.dog?via=matrix.asonix.dog) has done this.
3. Run pict-rs with `Nix`

Since I do my development for pict-rs on NixOS, I will document running pict-rs with Nix here.

### Installing with Nix

#### Install Nix

The official instructions [live here](https://nixos.org/download.html), but on Ubuntu you can follow
these steps:
```bash
$ sudo apt update
$ sudo apt install curl xz-utils
$ sh <(curl -L https://nixos.org/nix/install) --daemon
```

The Nix installer will ask if it's okay for it to make the changes it wants to make, and it will
print detailed logs about what it's doing.

After you get nix installed, we need to enable some nix features.

Open up `/etc/nix/nix.conf` in your favorite text editor (vim) and add the following line:
```
experimental-features = nix-command flakes
```

#### Build pict-rs

Now that nix is installed and configured, we can download and build the pict-rs nix package.

We'll fetch the latest code in the v0.4.x branch with git. This branch holds the latest changes
intended for releases in the 0.4 cycle.
```bash
$ sudo apt install git
$ git clone -b v0.4.x https://git.asonix.dog/asonix/pict-rs
```

And then we'll build the pict-rs nix package.
```bash
$ cd pict-rs
$ nix build
```

This will create a nix package with pict-rs and it's dependencies (exiftool, ffmpeg, and
imagemagick). You can see the contents of the package in the `result` symlink that was created by
the `nix build` command.

```bash
$ ls -lh | grep result
lrwxrwxrwx 1 asonix asonix   57 jul  9 19:47 result -> /nix/store/lblq0ns1p86qnpm3kd86ljpg2yx2i06b-pict-rs-0.4.1

$ ls result
bin

$ ls result/bin
pict-rs
```

> As an aside, this `pict-rs` file in `result/bin` is actually a shell script and not the binary.
    This script's purpose is to bring pict-rs' dependencies into the `$PATH` variable before
    invoking the real pict-rs binary. This is part of how Nix keeps applications isolated from each
    other while still allowing inter-package dependencies to exist.

#### Configuring systemd

Depending on when you follow these instructions, the produced pict-rs binary may have a different
path in the nix store. This is expected.

Now that we have a binary, we can configure it to start with `systemd`. This means writing a unit
file that will start the pict-rs binary when the machine boots. We have a couple options for this,
so I'll talk about both here.

Before we do any of that, let's go ahead and write the start of our unit file. Open a new file
called `pict-rs.service`
```service
[Unit]
Description=A simple image host
Documentation=https://git.asonix.dog/asonix/pict-rs
After=network-online.target
```

This just sets up some metadata and tells the operating system to wait until the network has been
brought up before starting pict-rs.

After the `[Unit]` section, we'll add a new section called `[Service]`. This describes how to launch
pict-rs, and when to restart it if needed. We'll need that symlink path from earlier for this step,
too.

```service
[Service]
Type=simple
ExecStart=/nix/store/lblq0ns1p86qnpm3kd86ljpg2yx2i06b-pict-rs-0.4.1/bin/pict-rs run
Restart=on-failure
```

These are the minimum required fields to launch pict-rs, but it probably won't run how you'd like.
We'll configure pict-rs next

##### Adding configuration to the Unit File

This is the easier route, and will keep all the configuration in one file. In the same service file,
in the same `[Service]` section, we'll set some environment variables.

```service
Environment="PICTRS__SERVER__ADDRESS=127.0.0.1:8080"
Environment="PICTRS__SERVER__API_KEY=SOME-REALLY-SECRET-KEY"
Environment="PICTRS__TRACING__LOGGING__TARGETS=warn"
Environment="PICTRS__MEDIA__FORMAT=avif"
Environment="PICTRS__REPO__PATH=/var/lib/pict-rs/sled"
Environment="PICTRS__REPO__EXPORT_PATH=/var/lib/pict-rs/sled"
Environment="PICTRS__STORE__PATH=/var/lib/pict-rs/files"
```

This tells pict-rs to run just on the local box on part 8080, sets an api key for access to the
internel endpoints, reduces the log output to just warnings and errors, tells pict-rs to
automatically convert uploaded images to avif, and sets the directories for pict-rs' state to
`/var/lib/pict-rs`.

In all, our unit file should look like this:
```service
[Unit]
Description=A simple image host
Documentation=https://git.asonix.dog/asonix/pict-rs
After=network-online.target

[Service]
Type=simple
ExecStart=/nix/store/lblq0ns1p86qnpm3kd86ljpg2yx2i06b-pict-rs-0.4.1/bin/pict-rs run
Restart=on-failure
Environment="PICTRS__SERVER__ADDRESS=127.0.0.1:8080"
Environment="PICTRS__SERVER__API_KEY=SOME-REALLY-SECRET-KEY"
Environment="PICTRS__TRACING__LOGGING__TARGETS=warn"
Environment="PICTRS__MEDIA__FORMAT=avif"
Environment="PICTRS__REPO__PATH=/var/lib/pict-rs/sled-repo"
Environment="PICTRS__REPO__EXPORT_PATH=/var/lib/pict-rs/exports"
Environment="PICTRS__STORE__PATH=/var/lib/pict-rs/files"
```

Once the unit file is ready, save it to `/etc/systemd/system/pict-rs.service`.


##### Adding a dedicated pict-rs configuration file

Instead of configuring pict-rs with environment variables, we can instead use a configuration file.
First, we'll update our `ExecStart` entry to tell pict-rs to load the configuration file. 

```service
ExecStart=/nix/store/lblq0ns1p86qnpm3kd86ljpg2yx2i06b-pict-rs-0.4.1/bin/pict-rs -c /etc/pict-rs.toml run
```

Our full service file should now look like this:
```service
[Unit]
Description=A simple image host
Documentation=https://git.asonix.dog/asonix/pict-rs
After=network-online.target

[Service]
Type=simple
ExecStart=/nix/store/lblq0ns1p86qnpm3kd86ljpg2yx2i06b-pict-rs-0.4.1/bin/pict-rs -c /etc/pict-rs.toml run
Restart=on-failure
```

Save this service file to `/etc/systemd/system/pict-rs.service`

Now, we'll configure pict-rs with toml. Open a new file at `/etc/pict-rs.toml`

We'll add the following configuration:
```toml
[server]
address = "127.0.0.1:8080"
api_key = "SOME-REALLY-SECRET-KEY"

[tracing.logging]
targets = "warn"

[media]
format = "avif"

[repo]
path = "/var/lib/pict-rs/sled-repo"
export_path = "/var/lib/pict-rs/exports"

[store]
path = "/var/lib/pict-rs/files"
```

After saving that configuration file, we're ready to start pict-rs.


#### Starting pict-rs

Now that we have created a unit file for pict-rs, we are able to start the service. You can do this
with the following commands:
```bash
$ sudo systemctl daemon-reload
$ sudo systemctl enable --now pict-rs
```

If everything went well, `pict-rs` should now be running on your system. You can follow its logs
with this command:
```bash
$ journalctl -xfu pict-rs
```

I hope this has been helpful to Ubuntu and Debian server admins. If you are familiar with packaging
software for Debian, consider stepping up to help maintain the ImageMagick package. There was a call
for help maintaining it last year on the [debian bug
tracker](https://bugs.debian.org/cgi-bin/bugreport.cgi?bug=1017366)
