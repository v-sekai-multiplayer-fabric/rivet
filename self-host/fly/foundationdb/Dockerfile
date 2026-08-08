# syntax=docker/dockerfile:1
#
# One fdbserver process per Fly machine.
#
#   docker build -f self-host/fly/foundationdb/Dockerfile -t fdb-fly self-host/fly/foundationdb

ARG FDB_VERSION=7.3.76

FROM foundationdb/foundationdb:${FDB_VERSION}

COPY entrypoint.sh /usr/local/bin/fdb-fly-entrypoint
RUN chmod +x /usr/local/bin/fdb-fly-entrypoint

ENTRYPOINT ["/usr/local/bin/fdb-fly-entrypoint"]
