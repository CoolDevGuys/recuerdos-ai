#!/bin/sh
# Runs as root (image default); fixes ownership of the named volumes
# (cargo registry/git cache, target dir) to the `dev` user, then drops
# privileges before exec'ing the real command. Without this, volumes
# created by the Docker daemon on first mount are root-owned and every
# cargo invocation as `dev` fails to write to them.
set -e

# /models holds the downloaded embedding model. Like the cargo volumes,
# a freshly created bind mount or named volume arrives root-owned, and
# fastembed then fails to write with a message that says nothing about
# permissions ("Failed to retrieve onnx/model.onnx").
mkdir -p /usr/local/cargo/registry /usr/local/cargo/git /app/target /models
chown -R dev:dev /usr/local/cargo/registry /usr/local/cargo/git /app/target /models

export HOME=/home/dev
exec setpriv --reuid=dev --regid=dev --init-groups "$@"
