#!/usr/bin/env bash
#
# Generate the certificate a WebTransport client can pin, and print its hash.
#
# WebTransport lets a client accept a self-signed certificate by its SHA-256
# hash via `serverCertificateHashes`, which avoids needing a CA, a domain, or
# DNS. The rules are narrow and a certificate that breaks any of them is
# refused outright rather than warned about:
#
#   * ECDSA over P-256. An RSA key is rejected.
#   * Valid for less than two weeks. A pinned hash cannot be revoked, so the
#     short life is what limits the damage if the key leaks.
#
# Let's Encrypt cannot serve this purpose: its shortest certificate is 90 days,
# well past the ceiling. That is not a gap to work around, it is the wrong tool.
#
#   bash self-host/webtransport/gen-cert.sh [output-dir] [common-name]
#
# Guard wants four paths, so the same pair is written under both the actor and
# api names. Splitting them matters when the actor and API are on separate
# hostnames, which a local run is not.

set -euo pipefail

OUT_DIR="${1:-./.webtransport-certs}"
CN="${2:-localhost}"

# Thirteen days, not fourteen. A certificate generated just before midnight
# should not expire early on the last day because of rounding.
DAYS=13

mkdir -p "${OUT_DIR}"

key="${OUT_DIR}/cert.key"
crt="${OUT_DIR}/cert.crt"

openssl ecparam -name prime256v1 -genkey -noout -out "${key}"

openssl req -new -x509 \
	-key "${key}" \
	-out "${crt}" \
	-days "${DAYS}" \
	-subj "/CN=${CN}" \
	-addext "subjectAltName=DNS:${CN},DNS:localhost,IP:127.0.0.1"

# Guard reads four paths. The actor and API certificates differ only when they
# serve different hostnames.
for role in actor api; do
	cp "${crt}" "${OUT_DIR}/${role}.crt"
	cp "${key}" "${OUT_DIR}/${role}.key"
done

chmod 600 "${OUT_DIR}"/*.key

hash_hex="$(openssl x509 -in "${crt}" -outform der | openssl dgst -sha256 | awk '{print $2}')"
hash_b64="$(openssl x509 -in "${crt}" -outform der | openssl dgst -sha256 -binary | base64)"

cat <<EOF

Wrote ${OUT_DIR}
  expires   $(openssl x509 -in "${crt}" -noout -enddate | cut -d= -f2)
  key type  $(openssl x509 -in "${crt}" -noout -text | grep 'NIST CURVE' | tr -s ' ')

Point Guard at it:
  RIVET__GUARD__HTTPS__PORT=443
  RIVET__GUARD__HTTPS__TLS__ACTOR_CERT_PATH=${OUT_DIR}/actor.crt
  RIVET__GUARD__HTTPS__TLS__ACTOR_KEY_PATH=${OUT_DIR}/actor.key
  RIVET__GUARD__HTTPS__TLS__API_CERT_PATH=${OUT_DIR}/api.crt
  RIVET__GUARD__HTTPS__TLS__API_KEY_PATH=${OUT_DIR}/api.key

Pin it from the browser:
  new WebTransport(url, {
    serverCertificateHashes: [{
      algorithm: "sha-256",
      value: Uint8Array.from(atob("${hash_b64}"), c => c.charCodeAt(0)),
    }],
  })

  sha-256 (hex) ${hash_hex}

Regenerate before the expiry above, or the client will refuse the connection
with no useful diagnostic.
EOF
