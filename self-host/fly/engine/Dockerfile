# syntax=docker/dockerfile:1
#
# Rivet Engine built against FoundationDB.
#
# Build from the rivet repo root:
#   docker build -f self-host/fly/engine/Dockerfile -t rivet-engine-fdb .
#
# The FDB client library is a hard runtime dependency of the foundationdb
# feature, and its version must track the cluster's server version.

ARG FDB_VERSION=7.3.76
ARG RUST_VERSION=1

FROM foundationdb/foundationdb:${FDB_VERSION} AS fdb

FROM rust:${RUST_VERSION}-bookworm AS builder

ARG FDB_VERSION
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates clang cmake libclang-dev libssl-dev pkg-config protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# The build needs libfdb_c.so to link against. The headers come from the crate's
# embedded-fdb-include feature, so only the library is copied here.
COPY --from=fdb /usr/lib/libfdb_c.so /usr/lib/libfdb_c.so

WORKDIR /build
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release -p rivet-engine --bin rivet-engine --features foundationdb \
    && cp target/release/rivet-engine /rivet-engine

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=fdb /usr/lib/libfdb_c.so /usr/lib/libfdb_c.so
COPY --from=builder /rivet-engine /usr/local/bin/rivet-engine

RUN mkdir -p /etc/foundationdb

EXPOSE 6420

ENTRYPOINT ["/usr/local/bin/rivet-engine"]
CMD ["start"]
