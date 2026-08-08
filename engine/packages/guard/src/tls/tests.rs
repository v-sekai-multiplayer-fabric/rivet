use std::io::Write;
use std::process::Command;

use super::{CertSlot, load_certified_key};

/// Generate an ECDSA P-256 pair, matching what `self-host/webtransport/gen-cert.sh`
/// produces, so these tests exercise the same key type the deployment uses.
fn generate_pair(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
	let key = dir.join("test.key");
	let cert = dir.join("test.crt");

	let ok = Command::new("openssl")
		.args(["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out"])
		.arg(&key)
		.status()
		.expect("openssl must be installed to run these tests")
		.success();
	assert!(ok, "key generation failed");

	let ok = Command::new("openssl")
		.args(["req", "-new", "-x509", "-days", "1", "-subj", "/CN=localhost", "-key"])
		.arg(&key)
		.arg("-out")
		.arg(&cert)
		.status()
		.expect("openssl req failed to run")
		.success();
	assert!(ok, "certificate generation failed");

	(cert, key)
}

#[test]
fn a_valid_pair_loads() {
	let dir = tempfile::tempdir().unwrap();
	let (cert, key) = generate_pair(dir.path());

	load_certified_key(&cert, &key).expect("a freshly generated pair must load");
}

#[test]
fn an_empty_slot_serves_nothing() {
	// This is the state between the listeners binding and ACME finishing. It
	// must be distinguishable from "no HTTPS configured", which is what makes
	// the QUIC listener bind at all.
	let slot = CertSlot::new();
	assert!(slot.get().is_none());
}

#[test]
fn a_loaded_slot_serves_the_certificate() {
	let dir = tempfile::tempdir().unwrap();
	let (cert, key) = generate_pair(dir.path());

	let slot = CertSlot::new();
	slot.load_from_paths(&cert, &key).unwrap();

	assert!(slot.get().is_some());
}

#[test]
fn a_missing_certificate_errors_rather_than_yielding_nothing() {
	// Silently continuing would disable TLS and HTTP/3 together, which is the
	// failure this module exists to remove.
	let dir = tempfile::tempdir().unwrap();
	let missing = dir.path().join("absent.crt");
	let key = dir.path().join("absent.key");

	assert!(load_certified_key(&missing, &key).is_err());
}

#[test]
fn a_malformed_key_errors() {
	let dir = tempfile::tempdir().unwrap();
	let (cert, _key) = generate_pair(dir.path());

	let bad_key = dir.path().join("bad.key");
	let mut f = std::fs::File::create(&bad_key).unwrap();
	f.write_all(b"-----BEGIN PRIVATE KEY-----\nnot base64\n-----END PRIVATE KEY-----\n")
		.unwrap();

	assert!(load_certified_key(&cert, &bad_key).is_err());
}

#[test]
fn a_certificate_swapped_in_later_is_visible() {
	// ACME writes after the listeners are already serving, so a reader holding
	// the slot must observe the new certificate without being rebuilt.
	let dir = tempfile::tempdir().unwrap();
	let (cert, key) = generate_pair(dir.path());

	let slot = CertSlot::new();
	let reader = slot.clone();
	assert!(reader.get().is_none());

	slot.load_from_paths(&cert, &key).unwrap();
	assert!(reader.get().is_some());
}
