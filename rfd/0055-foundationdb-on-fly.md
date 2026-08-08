# RFD 0055 — FoundationDB-backed Rivet on Fly.io

## Summary

Rivet's OSS engine ships two storage backends, Postgres and a RocksDB-backed
file system. FoundationDB is documented as enterprise-only and has no
implementation in this tree. This RFD adds an FDB backend to UniversalDB and
describes what it takes to run the resulting stack on Fly.io alongside a
headless Godot zone.

Status: implemented behind a Cargo feature and deployed. The driver is verified
against a real FDB 7.3.76 cluster, and a three-machine `double`-redundancy
cluster is running on Fly in `sjc` with a fault tolerance of one machine. The
open risks are called out under [Operational risks](#operational-risks).

## Motivation

The stated goal was a Fly deployment using FDB, Rivet, and a Godot Linux
container. Two parts of that were not possible as written:

1. `engine/packages/config/src/config/db/mod.rs` defined
   `Database { Postgres, FileSystem }` with `deny_unknown_fields`, and
   `engine/packages/universaldb/src/driver/` held only `postgres/` and
   `rocksdb/`. No configuration could route the engine at an FDB cluster.
2. The Godot demo's base image,
   `ghcr.io/v-sekai-multiplayer-fabric/zone-godot-runtime`, is not anonymously
   pullable at either `latest` or the pinned engine tag.

The second is resolved by building on the upstream Godot release. The first is
what most of this RFD is about.

## Why FDB was the cheapest backend to add, not the most expensive

UniversalDB's public surface is a copy of the FoundationDB Rust bindings.
`KeySelector`, `RangeOption`, `StreamingMode`, `MutationType`,
`ConflictRangeType`, `Priority`, and the `Slice`/`Value`/`Values` types are
field-for-field the same, several with the upstream Apache/MIT header still on
them. `range_option.rs` carries the foundationdb-rs copyright. `value.rs`
comments a type as existing "to match FoundationDB API".

The residue is visible elsewhere too:

- `engine/packages/config/src/lib.rs` registers an env list-parse key for
  `foundationdb.addresses`, a config path that had no struct behind it.
- `engine/packages/universaldb/src/utils/mod.rs` defines
  `error_is_transaction_too_large` returning a hardcoded `false` with the
  comment "Only implemented with fdb".
- `engine/packages/universaldb/src/error.rs` marks `TransactionTooOld` as
  "TODO: Implement in rocksdb and postgres drivers", i.e. the abstraction was
  designed around FDB semantics and the other two drivers are the approximations.

So the FDB driver is largely passthrough. The Postgres driver needs a
`bytearange` type, a GiST exclusion constraint, and a GC task to emulate
serializable conflict ranges; the RocksDB driver needs a conflict tracker and a
per-transaction task. FDB needs neither, because it provides those semantics
natively.

## Design

### Driver

`engine/packages/universaldb/src/driver/fdb/` with three files mirroring the
existing drivers:

- `database.rs` — `FdbDatabaseDriver`, cluster-file handling, retry loop.
- `transaction.rs` — `FdbTransactionDriver`, the type mapping.
- `error.rs` — `FdbDriverError`, which preserves the FDB error code across the
  conversion into `anyhow::Error`.

Three details are worth recording.

**The network thread is process-global and never stops.** `foundationdb::boot()`
may be called once per process and returns a `NetworkAutoStop` guard whose
`Drop` calls `fdb_stop_network()` and aborts the process on failure. Dropping it
while a `Database` handle is still open is a crash. It lives in a
`static OnceLock<NetworkAutoStop>`; statics are never dropped, which is the
behavior we want and avoids a `Box::leak`.

**Commit consumes the transaction, but the trait does not.** `TransactionDriver`
exposes `commit_ref(&self)` while `foundationdb::Transaction::commit(self)`
takes ownership. The handle is held as
`parking_lot::Mutex<Option<Arc<Transaction>>>`; commit takes the `Option` and
`Arc::try_unwrap`s it. `parking_lot` is correct here rather than `tokio::sync`
because `set`, `clear`, `atomic_op`, and `cancel` are sync `&self` trait
methods. A commit racing an open range stream fails loudly rather than silently
skipping the commit.

**FDB classifies its own errors.** The retry loop prefers
`FdbError::is_retryable()` and `is_maybe_committed()` over the generic
`DatabaseError` mapping, falling back to the latter for errors raised by user
closures. The verdict is resolved into a plain value before the backoff await,
because a borrowed `dyn Error` is not `Sync` and would make the future non-`Send`.

### Configuration

```json
{ "foundationdb": { "addresses": ["[fdaa:0:1::3]:4500"] } }
```

or `RIVET__FOUNDATIONDB__ADDRESSES=[fdaa:0:1::3]:4500,[fdaa:0:1::4]:4500`,
which the pre-existing list-parse key already splits on commas.

`cluster_file` takes an existing `fdb.cluster` directly. `addresses` instead
generates one at `cluster_file_write_path`, which is what the Fly deployment
uses, because coordinator addresses are only known after the machines exist.

### Build gating

The driver is behind a default-off `foundationdb` feature on `universaldb`,
forwarded through `rivet-pools` and `rivet-engine`. `libfdb_c.so` is required at
both build and run time, and a default-on feature would break every build on a
machine without FDB installed. Selecting `FoundationDb` in config on a binary
built without the feature is an explicit error, not a silent fallback.

The `foundationdb` crate normally reads `fdb.options` from
`/usr/include/foundationdb`, which the runtime-only client package does not
install. The `embedded-fdb-include` feature vendors it and is enabled in the
workspace dependency.

## Fly topology

Three apps in one org, on the 6PN private network.

```
  mf-rivet-godot ──► mf-rivet-engine ──► mf-rivet-fdb
  (container-runner   (rivet-engine,      (fdbserver ×3,
   + headless Godot)   public :6420)       6PN only, volumes)
```

`mf-rivet-fdb` has no public IP. `mf-rivet-engine` is the only app with a public
service.

### What Fly makes hard

**6PN is IPv6-only.** FDB handles IPv6, but every address in a cluster file must
be bracketed (`[fdaa:0:1::3]:4500`). `fdbserver` is started with an explicit
`--public-address` set from `FLY_PRIVATE_IP` and `--listen-address [::]:4500`;
`auto` resolves from the cluster file and picks the wrong interface.

**Coordinators are addressed by IP, and Fly assigns IPs to machines.** This is
the central operational constraint. A Fly machine keeps its 6PN address for its
lifetime, so a fixed set of machines gives stable coordinators, but destroying
and recreating a machine changes the address and requires a `coordinators`
reconfiguration. Deploys that replace machines in place preserve the address;
`fly machine destroy` does not. This is why the FDB app's fly.toml exists mainly
to pin machine identity, and why scaling is done by cloning rather than by
letting Fly recreate.

**The cluster file is mutable state.** `fdbserver` rewrites it when the
coordinator set changes. The entrypoint seeds it only when absent, so a redeploy
cannot clobber a live coordinator set with a stale env var.

**Bootstrap is inherently two-phase, and the naive version silently splits the
cluster.** Coordinator addresses are not knowable until the machines exist, so a
machine created before `FDB_COORDINATORS` is set writes a cluster file naming
only itself. Three fresh machines therefore come up as three independent
one-node clusters, each reporting `FDBD joined cluster`, which looks healthy.
This was observed during the first deployment.

The entrypoint's normal rule is to seed the cluster file only when absent,
because `fdbserver` rewrites it whenever the coordinator set changes and a
redeploy must not clobber a live set. `FDB_FORCE_COORDINATORS=1` overrides that
for exactly one deploy, which is what merges the split clusters.
`deploy.sh` sets it, redeploys, clears it, and only then runs `configure new`.
Verify by diffing `/var/fdb/fdb.cluster` across machines before configuring.

**Machines must not autostop.** A suspended coordinator takes the cluster down.
The FDB app runs with autostop disabled and `min_machines_running` at the
machine count.

**Redundancy needs real machines.** `double` requires three processes and
tolerates one loss; `single` is one machine with no fault tolerance. Fly gives
no hard anti-affinity guarantee, so three machines in one region may share a
host. For a deployment that must survive host loss, spread regions and accept
the cross-region latency on every transaction.

### Client/server version coupling

`libfdb_c.so` in the engine image must be protocol-compatible with the
`fdbserver` version. Both are pinned to 7.3.76 and copied from the same
`foundationdb/foundationdb:7.3.76` image, so they cannot drift independently.
Upgrading the cluster means rebuilding the engine image in the same change, or
configuring the client's multi-version fallback, which this RFD does not set up.

## Godot zone

The demo now builds on `Godot_v4.7.1-stable_linux.x86_64` from the upstream
release. Godot 4 ships a single Linux binary that runs headless under
`--headless`, so there is no separate server download. `libfontconfig1` and
`libfreetype6` are still linked under `--headless`. `HOME` is set because Godot
writes config and an import cache there, and the project is imported at build
time so the first actor start does not pay for it.

The `vsekai-godot-mcp` addon is fetched from a public GitHub tarball, which is
independent of the inaccessible GHCR image, so it is unchanged at its pinned
commit.

The demo README previously recorded that none of this had ever been executed and
flagged two specific uncertainties. Both are now resolved by running it:

- `WebSocketPeer.accept_stream` inside the polling loop works; a raw handshake
  returns `101 Switching Protocols` and `echo: hello` comes back.
- `_cmds.root` resolves usefully under `--script`; `tools/list` over MCP returns
  the full tool catalog.

## Operational risks

These are known and unresolved.

1. **Machine replacement breaks the coordinator set.** Nothing here reconciles a
   changed 6PN address automatically. A destroyed FDB machine needs a manual
   `fdbcli coordinators` run. A reconciler that reads machine addresses and
   applies them would remove the sharpest edge.
2. **`configure new` is destructive and only tolerated as a no-op on re-run.**
   `deploy.sh` swallows its failure so re-runs work, which also means a genuine
   misconfiguration is easy to miss. Read the `status minimal` output.
3. **No backups.** FDB backup to object storage is not configured. The volumes
   are the only copy.
4. **Not load tested.** The driver is correctness-tested, not benchmarked, and
   FDB's five-second transaction limit interacts with Rivet's `TXN_TIMEOUT` in
   ways this RFD has not measured under load.
5. **Shared-host redundancy.** As above, `double` on three same-region machines
   may not survive a single host failure.
6. **The VMs are undersized.** `shared-cpu-2x` with 2 GB gives FDB 1.8 GB per
   process against its 4 GB recommendation, and the cluster says so in `status`.
   It runs, but this is not a load-bearing configuration.

## Deployment results

The stack was deployed to the `personal` org in `sjc`.

- `mf-rivet-fdb` — 3 machines, 3 zones, `double` redundancy, fault tolerance of
  1 machine. `status` reports the database available.
- `mf-rivet-engine` — starts with
  `database: Some(FoundationDb(...))` naming the three coordinators, creates the
  default namespace, completes every startup backfill workflow, and serves
  `{"runtime":"engine","status":"ok","version":"2.3.7"}` on `/health`.

The Godot zone runs as a Rivet actor end to end. Creating an actor cold starts
the container, Godot boots and prints its ready line, the envoy connects, and an
MCP call through the gateway reaches the live SceneTree:

```
POST https://mf-rivet-engine.fly.dev/request/mcp
  x-rivet-target: actor
  x-rivet-actor: 57c13zaqureq7pl93wm2qtcqmqbl00

{"result":{"content":[{"text":"{\"engine\":{...\"string\":\"4.7.1-stable (official)\"},\"pong\":true}"}]}}
```

One measurement worth keeping: the first UDB read after startup,
`engine_check_version_rollback`, logged `slow udb operation ... duration_ms=2200`.
That is cold-start cost on an undersized cluster, not steady state, but it is the
only latency number this RFD has and it is not a good one. Treat FDB latency on
Fly as unmeasured until someone benchmarks it properly.

### The engine must advertise a reachable endpoint

`pegboard-outbound` sends the current datacenter's `public_url` to each envoy as
`x-rivet-endpoint`, and the envoy dials it to open its WebSocket back. The
default is `http://127.0.0.1:6420`, which an envoy resolves inside its own
container, so every connection attempt fails with `Connection refused` while the
engine itself looks healthy. It must be the engine's 6PN address.

Two traps sit on top of that:

- `topology.datacenters` deserializes through an untagged enum, so the env-var
  source cannot merge into it. Setting `RIVET__TOPOLOGY__DATACENTERS__DEFAULT__*`
  fails startup with `failed to deserialize config`, even with every required
  field supplied. It has to be a config file.
- In the map form, `name` is derived from the key, and setting it explicitly is
  rejected with a validation error.

A `[[files]]` block in `fly.toml` did not apply on deploy;
`flyctl machine update --file-literal` does, and is what `deploy.sh` uses.

## Testing

`engine/packages/universaldb/tests/fdb.rs` covers set/get roundtrip, missing
keys, ordered range scans, atomic `Add` accumulation, clears, and conflict
ranges. It skips unless `FDB_CLUSTER_FILE` is set, so the suite still passes
without FDB installed.

```
FDB_CLUSTER_FILE=... cargo test -p universaldb --features foundationdb --test fdb
```

All six pass against FoundationDB 7.3.76.

Note that `engine/packages/universaldb/tests/integration.rs` does not compile on
this branch, and did not before this change either. It references
`universaldb::options::DatabaseOption` and `Database::set_option`, neither of
which exists. That is out of scope here but blocks a bare
`cargo test -p universaldb`.

## Alternatives considered

**Postgres on Fly.** Supported today with no engine change, and the honest
recommendation for most deployments. Rejected because the request was
specifically FDB, and because the docs cap Postgres at roughly 1,000 concurrent
actors.

**Filesystem on a volume.** Simplest and cheapest, but single-node with no
failover, so it does not represent what FDB was being asked for.

**Fly Managed Postgres.** Same as the first option with less operational
surface. Still not FDB.
