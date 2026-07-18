#!/bin/sh
# Runs as root (image default); fixes ownership of the named volumes
# (cargo registry/git cache, target dir) to the `dev` user, then drops
# privileges before exec'ing the real command. Without this, volumes
# created by the Docker daemon on first mount are root-owned and every
# cargo invocation as `dev` fails to write to them.
set -e

mkdir -p /usr/local/cargo/registry /usr/local/cargo/git /app/target
chown -R dev:dev /usr/local/cargo/registry /usr/local/cargo/git /app/target

export HOME=/home/dev
exec setpriv --reuid=dev --regid=dev --init-groups "$@"
