use universaldb::prelude::*;

use super::{AcmeAccountKey, AcmeLockKey, CertChainKey, CertPrivateKey};

/// Pack a key the way a transaction would, so these assert the bytes that
/// actually reach the store.
fn packed<K: TuplePack>(key: &K) -> Vec<u8> {
	crate::keys::subspace().pack(key)
}

#[test]
fn a_pem_blob_round_trips_unchanged() {
	// The value codec is the identity on purpose: a PEM chain is already a
	// self-describing format owned by someone else, so wrapping it in a schema
	// would add a version to something that already has one.
	let key = CertChainKey::new();
	let pem = b"-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_vec();

	let stored = key.serialize(pem.clone()).unwrap();
	assert_eq!(key.deserialize(&stored).unwrap(), pem);
}

#[test]
fn a_lock_timestamp_round_trips() {
	let key = AcmeLockKey::new();
	let ts = 1_786_220_604_297i64;

	let stored = key.serialize(ts).unwrap();
	assert_eq!(key.deserialize(&stored).unwrap(), ts);
}

#[test]
fn every_key_lands_somewhere_different() {
	// A collision here would have one value silently overwrite another, and the
	// certificate and its private key are the pair most likely to be confused.
	let keys = [
		packed(&CertChainKey::new()),
		packed(&CertPrivateKey::new()),
		packed(&AcmeAccountKey::new()),
		packed(&AcmeLockKey::new()),
	];

	for (i, a) in keys.iter().enumerate() {
		for (j, b) in keys.iter().enumerate() {
			if i != j {
				assert_ne!(a, b, "keys {i} and {j} pack to the same bytes");
			}
		}
	}
}

#[test]
fn keys_sit_under_guards_own_subspace() {
	// Guard must not write inside pegboard's root, which orchestration GC
	// scans and clears wholesale.
	let subspace = crate::keys::subspace();
	let chain = packed(&CertChainKey::new());

	assert!(
		chain.starts_with(&subspace.pack(&())),
		"key escaped guard's subspace"
	);
}

#[test]
fn a_key_unpacks_back_to_itself() {
	// Round tripping through the subspace proves the root and leaf guards in
	// `unpack` accept what `pack` produced.
	let subspace = crate::keys::subspace();
	let packed_chain = subspace.pack(&CertChainKey::new());

	subspace
		.unpack::<CertChainKey>(&packed_chain)
		.expect("a chain key must unpack as a chain key");
}

#[test]
fn a_key_does_not_unpack_as_a_different_key() {
	// The leaf guard exists so a mismatched read fails loudly rather than
	// returning another key's bytes.
	let subspace = crate::keys::subspace();
	let packed_chain = subspace.pack(&CertChainKey::new());

	assert!(
		subspace.unpack::<AcmeLockKey>(&packed_chain).is_err(),
		"a chain key must not unpack as a lock key"
	);
}
