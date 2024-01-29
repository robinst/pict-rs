# How to prepare postgres for pict-rs

## Preparing postgres

### I already have a postgres

If you already have a postgres server, we'll create an additional database in it for pict-rs. First,
you'll need to connect to your postgres server as an administrator. The general command should look
like the following.
```bash
psql -U postgres
```
> `postgres` here is the name of the administrator role. If your administrator role is named
something else (maybe `lemmy`) then use that instead.

If postgres is running in a docker-compose environment, it might look like this.
```bash
sudo docker-compose exec postgres psql -U postgres
```
> note that the first `postgres` in this command is the docker-compose service, and the second
`postgres` is the name of the database administrator role

Once you have a postgres shell, we'll configure the postgres user and database.

First, create the pictrs user.
```sql
CREATE USER pictrs;
```

Then set the pictrs user's password
```sql
\password pictrs -- allows setting the password for the postgres user
```

Finally, create the pictrs database giving ownership to the pictrs user.
```sql
CREATE DATABASE pictrs OWNER pictrs;
```

The database configuration is now complete.

### I don't have a postgres

Postgres can be installed in a variety of ways, but a simple way to do it is with docker-compose,
although installing docker and docker-compose is left as an exercise to the reader. An example
docker-compose file can be found below.

```yaml
version: '3.3'

services:
  postgres:
    image: postgres:16-alpine
    ports:
    - "5432:5432"
    environment:
    - PG_DATA=/var/lib/postgresql/data
    - POSTGRES_DB=pictrs
    - POSTGRES_USER=pictrs
    - POSTGRES_PASSWORD=CREATE_YOUR_OWN_PASSWORD
    volumes:
    - ./storage/postgres:/var/lib/postgresql/data
```

After the file is written, a quick `sudo docker-compose up -d` should launch the postgres server,
making it available on `localhost:5432`.

## Connecting to postgres

pict-rs can be configured to talk to your postgres server a few different ways.
1. environment variables
2. the `pict-rs.toml` file
3. commandline arguments

In many cases, environment variables will be the easiest.

### Environment Variables

The variables you'll need to set are the following
- `PICTRS__REPO__TYPE`
- `PICTRS__REPO__URL`

with a few optional variables for folks who have TLS involved
- `PICTRS__REPO__USE_TLS`
- `PICTRS__REPO__CERTIFICATE_FILE`

For a simple self-hosted postgres deployment, the following variables should be set:
```bash
PICTRS__REPO__TYPE=postgres
PICTRS__REPO__URL=postgres://pictrs:CREATE_YOUR_OWN_PASSWORD@localhost:5432/pictrs
```

If you're running pict-rs in the same docker-compose file as you are postgres, then change
`localhost` in the above URL to the name of your postgres service, e.g.
```yaml
- PICTRS__REPO__TYPE=postgres
- PICTRS__REPO__URL=postgres://pictrs:CREATE_YOUR_OWN_PASSWORD@postgres:5432/pictrs
```

If your postgres is provided by another party, or exists on a different host, then provide the
correct hostname or IP address to reach it.

If your postgres supports TLS connections, as might be present in cloud environments, then the
following variables should be set.
```bash
PICTRS__REPO__USE_TLS=true
PICTRS__REPO__CERTIFICATE_FILE=/path/to/certificate/file.crt
```
> Note that if you provide a path to the certificate file, pict-rs must be able to read that path.
This means that if you're running pict-rs in docker, the certificate file needs to be mounted into
the container.

### pict-rs.toml

The toml configuration can be set pretty easily. The `repo` section should look like the following
```toml
[repo]
type = 'postgres'
url = 'postgres://pictrs:CREATE_YOUR_OWN_PASSWORD@postgres:5432/pictrs'
```
note that the hostname `postgres` should be changed to the host that your postgres server is
accessible at.

For enabling TLS, the configuration would look like the following:
```toml
[repo]
type = 'postgres'
url = 'postgres://pictrs:CREATE_YOUR_OWN_PASSWORD@postgres:5432/pictrs'
use_tls = true
certificate_file = '/path/to/certificate/file.crt'
```

### Commandline arguments

pict-rs can be configured entirely from the commandline. An example invocation could look like the
following:
```bash
pict-rs run \
    filesytem -p /path/to/files \
    postgres -u 'postgres://pictrs:CREATE_YOUR_OWN_PASSWORD@postgres:5432/pictrs'
```

with TLS it could look like this:
```bash
pict-rs run \
    filesytem -p /path/to/files \
    postgres \
        -u 'postgres://pictrs:CREATE_YOUR_OWN_PASSWORD@postgres:5432/pictrs' \
        -t \
        -c /path/to/certificate/file.crt
```

## Additional comments

When configuring TLS, the `certificate_file` setting isn't required, however, it is likely it will
be used when TLS is enabled. A case when it might not be required is if your postgres publicly
accessible on the internet and receives a valid certificate from a trusted certificate authority.

For testing TLS, I've been using `certstrap` to generate a CA and certificates. I have a script
called `setup-tls.sh` that looks like this:
```bash
certstrap init --common-name pictrsCA
certstrap request-cert --common-name postgres --domain localhost
certstrap sign postgres --CA pictrsCA
```

This genrates a CA and uses that CA to sign a new certificate for `localhost`. Then I update
postgres' pg_hba.conf file to allow connections over TLS:
```pg_hba.conf
hostssl all all all cert clientcert=verify-full
```

Finally, I launch postgres with a custom commandline.
```
-c "ssl=on" \
-c "ssl_cert_file=/path/to/postgres.crt" \
-c "ssl_key_file=/path/to/postgres.key" \
-c "ssl_ca_file=/path/to/pictrsCA.crt" \
-c "ssl_crl_file=/path/to/pictrsCA.crl"
```
Alternatively, I could update the postgresql.conf file.
```postgresql.conf
ssl=on
ssl_cert_file=/path/to/postgres.crt
ssl_key_file=/path/to/postgres.key
ssl_ca_file=/path/to/pictrsCA.crt
ssl_crl_file=/path/to/pictrsCA.crl
```
