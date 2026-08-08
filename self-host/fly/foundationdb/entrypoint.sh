#!/bin/bash
# Runs one fdbserver process per machine on Fly.
#
# Fly's private network (6PN) is IPv6 only, so every address here is bracketed.
# A machine keeps its 6PN address for its whole lifetime, which is what lets a
# fixed set of machines act as stable coordinators.
set -euo pipefail

: "${FDB_PORT:=4500}"
: "${FDB_CLUSTER_FILE:=/var/fdb/fdb.cluster}"
: "${FDB_DATA_DIR:=/var/fdb/data}"
: "${FDB_LOG_DIR:=/var/fdb/logs}"
: "${FDB_CLUSTER_DESCRIPTION:=rivet}"
: "${FDB_CLUSTER_ID:=rivet}"
: "${FDB_CLASS:=unset}"

# Fly exposes the machine's own 6PN address here.
public_ip="${FLY_PRIVATE_IP:?FLY_PRIVATE_IP not set}"
public_address="[${public_ip}]:${FDB_PORT}"

# Coordinator addresses are not knowable before the machines exist, so the first
# machine bootstraps as its own sole coordinator. deploy.sh reads the addresses
# back and sets FDB_COORDINATORS for real once every machine is up.
: "${FDB_COORDINATORS:=${public_address}}"

mkdir -p "${FDB_DATA_DIR}" "${FDB_LOG_DIR}" "$(dirname "${FDB_CLUSTER_FILE}")"

# fdbserver rewrites the cluster file when coordinators change, so only seed it
# when it is missing. Otherwise a redeploy would clobber a live coordinator set.
#
# FDB_FORCE_COORDINATORS=1 overrides that. It is needed exactly once, to join
# machines that each bootstrapped as their own sole coordinator into one
# cluster, and must be unset afterwards.
if [ ! -s "${FDB_CLUSTER_FILE}" ] || [ "${FDB_FORCE_COORDINATORS:-0}" = "1" ]; then
	echo "${FDB_CLUSTER_DESCRIPTION}:${FDB_CLUSTER_ID}@${FDB_COORDINATORS}" > "${FDB_CLUSTER_FILE}"
fi

echo "starting fdbserver on ${public_address} (class=${FDB_CLASS})" >&2
cat "${FDB_CLUSTER_FILE}" >&2

exec fdbserver \
	--cluster-file "${FDB_CLUSTER_FILE}" \
	--datadir "${FDB_DATA_DIR}" \
	--logdir "${FDB_LOG_DIR}" \
	--public-address "${public_address}" \
	--listen-address "[::]:${FDB_PORT}" \
	--class "${FDB_CLASS}"
