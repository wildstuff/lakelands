#!/bin/sh
# Runs inside the postgres:18 container that scripts/pg-live/up.sh starts:
# installs the pinned PgBouncer, starts two of them, and execs the server.
# Ports: 56432 the server, 56433 PgBouncer in transaction mode, 56434 in
# session mode. /certs holds ca.pem, server.pem, server.key (read only).
set -eu
apt-get update -qq
apt-get install -y -qq --no-install-recommends "pgbouncer=$PGBOUNCER_VERSION" >/dev/null

# The server's key must belong to the database user and be private.
install -d -o postgres -g postgres -m 700 /tls
for f in ca.pem server.pem server.key; do
    install -o postgres -g postgres -m 600 "/certs/$f" "/tls/$f"
done

# `lakelands_mtls` demands a client certificate whose CN is the user, and
# accepts nothing else; every other database takes a password.
cat >/tls/pg_hba.conf <<HBA
local all all trust
hostssl lakelands_mtls lakelands 0.0.0.0/0 cert
hostssl lakelands_mtls lakelands ::/0 cert
host lakelands_mtls all all reject
host all all 0.0.0.0/0 scram-sha-256
host all all ::/0 scram-sha-256
HBA
chown postgres:postgres /tls/pg_hba.conf

for mode in transaction:56433 session:56434; do
    name=${mode%%:*}
    port=${mode##*:}
    d=/tmp/pgbouncer-$name
    install -d -o postgres -g postgres "$d"
    echo '"lakelands" "lakelands"' >"$d/users.txt"
    cat >"$d/pgbouncer.ini" <<INI
[databases]
lakelands = host=127.0.0.1 port=56432 dbname=lakelands
[pgbouncer]
listen_addr = 127.0.0.1
listen_port = $port
auth_type = scram-sha-256
auth_file = $d/users.txt
pool_mode = $name
default_pool_size = 4
ignore_startup_parameters = options,extra_float_digits
server_tls_sslmode = disable
logfile = $d/log
pidfile = $d/pid
INI
    chown -R postgres:postgres "$d"
    gosu postgres pgbouncer -d "$d/pgbouncer.ini"
done

# The server's own start may take a pooler's first connection before it is
# up; the poolers connect on demand, so none waits for it.
exec docker-entrypoint.sh postgres \
    -c port=56432 -c listen_addresses='*' \
    -c ssl=on -c ssl_cert_file=/tls/server.pem -c ssl_key_file=/tls/server.key \
    -c ssl_ca_file=/tls/ca.pem -c hba_file=/tls/pg_hba.conf
