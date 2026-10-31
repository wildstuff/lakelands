#!/usr/bin/env bash
# Starts the servers tests/live_servers.rs of lakelands-engine-pg runs
# against, in one container of the image CI's `pg` leg uses, and prints
# the variables that test file reads. Stop it with `docker rm -f lakelands-pg-live`.
# Usage: eval "$(scripts/pg-live/up.sh)"
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

IMAGE=mirror.gcr.io/library/postgres:18
# The PgBouncer the PostgreSQL project's apt repository carries for the
# image's Debian release. Raise it here, in a pull request of its own.
PGBOUNCER_VERSION=1.26.1-1.pgdg13+1
NAME=${LAKELANDS_PG_LIVE_NAME:-lakelands-pg-live}
DIR=$(mktemp -d)
chmod 755 "$DIR"

# A private authority; a server certificate for 127.0.0.1; a client
# certificate whose CN is the database user.
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=lakelands-live-ca \
    -keyout "$DIR/ca.key" -out "$DIR/ca.pem" 2>/dev/null
leaf() { # name subject extfile-content
    openssl req -newkey rsa:2048 -nodes -subj "/CN=$2" -keyout "$DIR/$1.key" \
        -out "$DIR/$1.csr" 2>/dev/null
    printf '%s\n' "$3" >"$DIR/$1.ext"
    openssl x509 -req -in "$DIR/$1.csr" -CA "$DIR/ca.pem" -CAkey "$DIR/ca.key" \
        -CAcreateserial -days 2 -extfile "$DIR/$1.ext" -out "$DIR/$1.pem" 2>/dev/null
}
leaf server localhost "subjectAltName=DNS:localhost,IP:127.0.0.1"
leaf client lakelands "extendedKeyUsage=clientAuth"
chmod 644 "$DIR"/*

docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" --network host \
    -e POSTGRES_USER=lakelands -e POSTGRES_PASSWORD=lakelands -e POSTGRES_DB=lakelands \
    -e PGBOUNCER_VERSION="$PGBOUNCER_VERSION" \
    -v "$DIR:/certs:ro" -v "$PWD/scripts/pg-live/container.sh:/container.sh:ro" \
    --entrypoint /container.sh "$IMAGE" >&2

# Ready when the server answers on its port and both poolers do.
for _ in $(seq 120); do
    if docker exec "$NAME" sh -c \
        'pg_isready -q -h 127.0.0.1 -p 56432 && pg_isready -q -h 127.0.0.1 -p 56433 && pg_isready -q -h 127.0.0.1 -p 56434' \
        2>/dev/null; then
        break
    fi
    sleep 1
done
docker exec "$NAME" pg_isready -h 127.0.0.1 -p 56432 >&2
docker exec -e PGPASSWORD=lakelands "$NAME" psql -q -U lakelands -h 127.0.0.1 -p 56432 -d lakelands \
    -c 'CREATE DATABASE lakelands_mtls' >&2

U=postgres://lakelands:lakelands@127.0.0.1
cat <<VARS
export LAKELANDS_TEST_PG_LIVE=1
export LAKELANDS_TEST_PG_URL=$U:56432/lakelands
export LAKELANDS_TEST_PG_POOLER_TX_URL=$U:56433/lakelands
export LAKELANDS_TEST_PG_POOLER_SESSION_URL=$U:56434/lakelands
export LAKELANDS_TEST_PG_MTLS_URL=$U:56432/lakelands_mtls
export LAKELANDS_TEST_PG_MTLS_DIR=$DIR
VARS
