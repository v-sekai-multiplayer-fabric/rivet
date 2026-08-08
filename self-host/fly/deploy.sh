#!/usr/bin/env bash
# Brings up the FoundationDB cluster, Rivet Engine, and the Godot zone on Fly.
#
# Coordinator addresses are only knowable after the FDB machines exist, so this
# runs in phases: create machines, read their 6PN addresses back, then configure
# the cluster and point the engine at it.
#
# Re-running is safe. Each phase checks for what it is about to create.
set -euo pipefail

FDB_APP="${FDB_APP:-mf-rivet-fdb}"
ENGINE_APP="${ENGINE_APP:-mf-rivet-engine}"
GODOT_APP="${GODOT_APP:-mf-rivet-godot}"
REGION="${REGION:-sjc}"
ORG="${ORG:-personal}"
FDB_PORT="${FDB_PORT:-4500}"
# 1 machine allows `configure single`. Use 3 for `double` redundancy.
FDB_COUNT="${FDB_COUNT:-3}"
FDB_REDUNDANCY="${FDB_REDUNDANCY:-double}"
FDB_STORAGE="${FDB_STORAGE:-ssd}"
VOLUME_SIZE_GB="${VOLUME_SIZE_GB:-10}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

app_exists() { flyctl status --app "$1" >/dev/null 2>&1; }

# --- FoundationDB -----------------------------------------------------------

log "FoundationDB app"
if ! app_exists "${FDB_APP}"; then
	flyctl apps create "${FDB_APP}" --org "${ORG}"
fi

# No public IP. The cluster is reachable only over 6PN.
flyctl ips list --app "${FDB_APP}" 2>/dev/null | grep -q . || true

log "Building the FoundationDB image"
(cd self-host/fly/foundationdb && flyctl deploy . \
	--app "${FDB_APP}" \
	--regions "${REGION}" \
	--ha=false \
	--yes)

log "Ensuring ${FDB_COUNT} FDB machines"
current="$(flyctl machine list --app "${FDB_APP}" --json | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')"
while [ "${current}" -lt "${FDB_COUNT}" ]; do
	flyctl machine clone \
		"$(flyctl machine list --app "${FDB_APP}" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["id"])')" \
		--app "${FDB_APP}" --region "${REGION}"
	current=$((current + 1))
done

log "Reading coordinator addresses"
# A machine keeps its 6PN address for its lifetime, so these are stable as long
# as the machines are not destroyed.
coordinators="$(flyctl machine list --app "${FDB_APP}" --json \
	| python3 -c "
import json, sys
ms = json.load(sys.stdin)
addrs = [m['private_ip'] for m in ms if m.get('private_ip')]
addrs.sort()
print(','.join(f'[{a}]:${FDB_PORT}' for a in addrs[:${FDB_COUNT}]))
")"
echo "coordinators: ${coordinators}"

log "Setting FDB_COORDINATORS"
# Each machine bootstrapped as its own sole coordinator, because the addresses
# were not knowable at create time. FDB_FORCE_COORDINATORS makes the entrypoint
# overwrite those cluster files so the machines form one cluster instead of
# three. It is cleared immediately after, so later redeploys cannot clobber a
# live coordinator set.
flyctl secrets set --app "${FDB_APP}" \
	"FDB_COORDINATORS=${coordinators}" \
	"FDB_FORCE_COORDINATORS=1" \
	--stage
(cd self-host/fly/foundationdb && flyctl deploy . \
	--app "${FDB_APP}" \
	--yes)

flyctl secrets unset --app "${FDB_APP}" FDB_FORCE_COORDINATORS --stage

log "Configuring the database"
# The first configure creates the database. It is an error to run it twice, so
# the failure is tolerated on re-runs.
flyctl ssh console --app "${FDB_APP}" --command \
	"fdbcli -C /var/fdb/fdb.cluster --exec 'configure new ${FDB_REDUNDANCY} ${FDB_STORAGE}'" || true
flyctl ssh console --app "${FDB_APP}" --command \
	"fdbcli -C /var/fdb/fdb.cluster --exec 'coordinators ${coordinators//,/ }'" || true
flyctl ssh console --app "${FDB_APP}" --command \
	"fdbcli -C /var/fdb/fdb.cluster --exec 'status minimal'"

# --- Rivet Engine -----------------------------------------------------------

log "Rivet Engine app"
if ! app_exists "${ENGINE_APP}"; then
	flyctl apps create "${ENGINE_APP}" --org "${ORG}"
fi

admin_token="${RIVET_ADMIN_TOKEN:-$(python3 -c 'import secrets; print(secrets.token_urlsafe(32))')}"

log "Setting engine secrets"
flyctl secrets set --app "${ENGINE_APP}" \
	"RIVET__FOUNDATIONDB__ADDRESSES=${coordinators}" \
	"RIVET__AUTH__ADMIN_TOKEN=${admin_token}" \
	--stage

log "Deploying the engine"
flyctl deploy . \
	--app "${ENGINE_APP}" \
	--config self-host/fly/engine/fly.toml \
	--dockerfile self-host/fly/engine/Dockerfile \
	--regions "${REGION}" \
	--ha=false \
	--yes

log "Writing the engine topology file"
# The engine hands public_url to each envoy as x-rivet-endpoint, and the envoy
# dials it. The default is http://127.0.0.1:6420, which an envoy resolves inside
# its own container, so it must be the engine's 6PN address.
#
# This is a file because `topology.datacenters` deserializes through an untagged
# enum that the env-var source cannot merge into. `name` is omitted because it
# is derived from the map key and setting it is rejected.
topology_json="$(python3 -c "
import json
print(json.dumps({'topology': {'datacenter_label': 1, 'datacenters': {'default': {
    'datacenter_label': 1,
    'is_leader': True,
    'public_url': 'http://${ENGINE_APP}.internal:6420',
    'peer_url': 'http://${ENGINE_APP}.internal:6421',
}}}}, separators=(',', ':')))
")"

for machine in $(flyctl machine list --app "${ENGINE_APP}" --json \
	| python3 -c 'import json,sys; [print(m["id"]) for m in json.load(sys.stdin)]'); do
	flyctl machine update "${machine}" --app "${ENGINE_APP}" \
		--file-literal "/etc/rivet/topology.json=${topology_json}" --yes
done

engine_host="${ENGINE_APP}.fly.dev"

# --- Godot zone -------------------------------------------------------------

log "Godot zone app"
if ! app_exists "${GODOT_APP}"; then
	flyctl apps create "${GODOT_APP}" --org "${ORG}"
fi

log "Setting Godot zone secrets"
flyctl secrets set --app "${GODOT_APP}" \
	"RIVET_ENDPOINT=http://default:${admin_token}@${ENGINE_APP}.internal:6420" \
	--stage

log "Deploying the Godot zone"
flyctl deploy . \
	--app "${GODOT_APP}" \
	--config self-host/fly/godot-zone/fly.toml \
	--dockerfile container-runner/examples/godot-demo/Dockerfile \
	--regions "${REGION}" \
	--ha=false \
	--yes

log "Registering the runner"
# The URL carries container-runner's base path (RIVET_SERVERLESS_BASE_PATH),
# which the engine calls to start an actor.
curl -fsS -X PUT "https://${engine_host}/runner-configs/godot-zone?namespace=default" \
	-H "Authorization: Bearer ${admin_token}" \
	-H "Content-Type: application/json" \
	-d "{\"datacenters\":{\"default\":{\"serverless\":{\"url\":\"https://${GODOT_APP}.fly.dev/api/rivet\",\"request_lifespan\":900,\"drain_grace_period\":60,\"max_concurrent_actors\":4}}}}"

log "Done"
cat <<EOF

  engine dashboard  https://${engine_host}
  admin token       ${admin_token}
  fdb coordinators  ${coordinators}

EOF
