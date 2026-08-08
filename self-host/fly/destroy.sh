#!/usr/bin/env bash
#
# Tear the Fly deployment down and prove the bill is zero.
#
# Destroying the apps is not enough on its own to trust: a volume that outlives
# its app keeps billing at $0.15/GB per month, and three 10 GB volumes are $4.50
# a month for data nobody can reach. So this verifies afterwards rather than
# assuming, and exits non-zero if anything survived.
#
#   bash self-host/fly/destroy.sh          # asks first
#   bash self-host/fly/destroy.sh --yes    # does not
#
# This is irreversible. The volumes hold the FoundationDB data, and nothing is
# backed up unless RFD 0013 has been implemented.

set -euo pipefail

FDB_APP="${FDB_APP:-mf-rivet-fdb}"
ENGINE_APP="${ENGINE_APP:-mf-rivet-engine}"
GODOT_APP="${GODOT_APP:-mf-rivet-godot}"

APPS=("${GODOT_APP}" "${ENGINE_APP}" "${FDB_APP}")

log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

app_exists() { flyctl status --app "$1" >/dev/null 2>&1; }

if [[ "${1:-}" != "--yes" ]]; then
	log "About to destroy, permanently:"
	for app in "${APPS[@]}"; do
		app_exists "${app}" && printf '  %s\n' "${app}" || printf '  %s (absent)\n' "${app}"
	done
	printf '\nEvery volume goes with them, including the FoundationDB data.\n'
	read -r -p 'Type the word destroy to continue: ' reply
	[[ "${reply}" == "destroy" ]] || { echo "Aborted."; exit 1; }
fi

# The zone and engine go before FoundationDB. Removing the database out from
# under a running engine produces a burst of errors that make the logs harder to
# read than they need to be.
for app in "${APPS[@]}"; do
	if app_exists "${app}"; then
		log "Destroying ${app}"
		flyctl apps destroy "${app}" --yes
	else
		log "${app} does not exist, skipping"
	fi
done

log "Verifying nothing survived"

leftovers=0

for app in "${APPS[@]}"; do
	if app_exists "${app}"; then
		printf 'STILL PRESENT: app %s\n' "${app}"
		leftovers=1
	fi

	# A volume can outlive its app. Ask directly rather than trusting that
	# destroying the app took them with it.
	vols="$(flyctl volumes list --app "${app}" --json 2>/dev/null || echo '[]')"
	count="$(printf '%s' "${vols}" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))' 2>/dev/null || echo 0)"
	if [[ "${count}" != "0" ]]; then
		printf 'STILL PRESENT: %s volume(s) on %s\n' "${count}" "${app}"
		leftovers=1
	fi
done

if [[ "${leftovers}" != "0" ]]; then
	log "Teardown incomplete. The items above are still billing."
	exit 1
fi

log "All destroyed. Confirm at https://fly.io/dashboard, which is the only authority on the bill."
