# Rivet on Fly.io with FoundationDB

Three apps on one Fly org, connected over 6PN:

| App | What | Public |
|-----|------|--------|
| `mf-rivet-fdb` | `fdbserver`, one process per machine, volume-backed | No |
| `mf-rivet-engine` | `rivet-engine` built with the `foundationdb` feature | Yes, `:6420` |
| `mf-rivet-godot` | `container-runner` hosting a headless Godot MCP zone | Yes |

Design rationale and the operational risks live in
[rfd/0055-foundationdb-on-fly.md](../../rfd/0055-foundationdb-on-fly.md). Read
the risks before running this against anything you care about.

## Prerequisites

- `flyctl`, authenticated (`flyctl auth whoami`).
- Docker, for local image builds.
- An org with volume and machine quota for at least five machines.

## Deploy

```sh
./self-host/fly/deploy.sh
```

Override the defaults with env vars:

```sh
ORG=personal REGION=sea FDB_COUNT=3 FDB_REDUNDANCY=double ./self-host/fly/deploy.sh
```

`FDB_COUNT=1` with `FDB_REDUNDANCY=single` is the cheapest configuration and has
no fault tolerance.

The script prints the engine URL and the generated admin token at the end. The
token is also stored as the `RIVET__AUTH__ADMIN_TOKEN` secret on the engine app.

## What the script does

Coordinator addresses are not knowable until the FDB machines exist, so this
runs in phases:

1. Create and deploy the FDB app, then clone machines up to `FDB_COUNT`.
2. Read each machine's 6PN address back and set `FDB_COORDINATORS`, plus
   `FDB_FORCE_COORDINATORS=1` for one deploy.
3. Redeploy, clear the force flag, then `configure new <redundancy> <storage>`.
4. Deploy the engine with `RIVET__FOUNDATIONDB__ADDRESSES` pointed at them.
5. Deploy the Godot zone with `RIVET_ENDPOINT` pointed at the engine.
6. Register the zone as a serverless runner.

## Checking the cluster

```sh
flyctl ssh console --app mf-rivet-fdb --command \
  "fdbcli -C /var/fdb/fdb.cluster --exec 'status'"
```

`status minimal` should report the database as available. "Available, but has
issues" immediately after `configure new` usually means the cluster has not
finished recruiting; give it a minute.

## Checking the engine

```sh
curl https://mf-rivet-engine.fly.dev/health
```

## Gotchas

**Do not destroy FDB machines.** Coordinators are addressed by IP. A machine
keeps its 6PN address for its lifetime, but `flyctl machine destroy` followed by
a recreate gives a new address and the cluster file goes stale. Recover with:

```sh
flyctl ssh console --app mf-rivet-fdb --command \
  "fdbcli -C /var/fdb/fdb.cluster --exec 'coordinators [addr1]:4500 [addr2]:4500 [addr3]:4500'"
```

**New machines bootstrap alone.** A machine created before `FDB_COORDINATORS`
is known writes a cluster file naming only itself, so three fresh machines are
three separate one-node clusters rather than one. `FDB_FORCE_COORDINATORS=1`
makes the entrypoint overwrite that file, and `deploy.sh` sets it for exactly
one deploy then clears it. Verify with:

```sh
flyctl ssh console --app mf-rivet-fdb --command "cat /var/fdb/fdb.cluster"
```

All machines must print the same line.

**`shared-cpu-2x` is below what FDB wants.** The cluster reports a warning that
it has 1.8 GB per process against a 4 GB recommendation. It runs, but size the
VMs up before putting real load on it.

**Do not let FDB machines autostop.** A suspended coordinator takes the cluster
down. `fly.toml` keeps them running; do not add `auto_stop_machines`.

**The client and server versions are coupled.** Both are pinned to 7.3.76 and
copied from the same base image. Changing one without the other breaks the
connection with a protocol mismatch.

## Building images locally

```sh
# Engine, from the repo root
docker build -f self-host/fly/engine/Dockerfile -t rivet-engine-fdb .

# Godot zone, from the repo root
docker build -f container-runner/examples/godot-demo/Dockerfile -t godot-mcp-zone .

# FoundationDB
docker build -f self-host/fly/foundationdb/Dockerfile -t fdb-fly self-host/fly/foundationdb
```
