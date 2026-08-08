//! Keys holding the certificate Guard serves, and the ACME state behind it.
//!
//! Every value here is an opaque blob whose format belongs to something else: a
//! PEM chain, a PEM private key, or the `instant-acme` account credentials in
//! that crate's own encoding. None is a struct this repository defines, so none
//! needs a versioned schema. Expiry is read back out of the certificate rather
//! than stored beside it, which keeps it that way.

use anyhow::Result;
use universaldb::prelude::*;

/// The certificate chain Guard presents, PEM encoded.
#[derive(Debug)]
pub struct CertChainKey {}

impl CertChainKey {
	pub fn new() -> Self {
		Self {}
	}
}

impl Default for CertChainKey {
	fn default() -> Self {
		Self::new()
	}
}

impl FormalKey for CertChainKey {
	type Value = Vec<u8>;

	fn deserialize(&self, raw: &[u8]) -> Result<Self::Value> {
		Ok(raw.to_vec())
	}

	fn serialize(&self, value: Self::Value) -> Result<Vec<u8>> {
		Ok(value)
	}
}

impl TuplePack for CertChainKey {
	fn pack<W: std::io::Write>(
		&self,
		w: &mut W,
		tuple_depth: TupleDepth,
	) -> std::io::Result<VersionstampOffset> {
		let t = (TLS, CHAIN);
		t.pack(w, tuple_depth)
	}
}

impl<'de> TupleUnpack<'de> for CertChainKey {
	fn unpack(input: &[u8], tuple_depth: TupleDepth) -> PackResult<(&[u8], Self)> {
		let (input, (root, leaf)) = <(usize, usize)>::unpack(input, tuple_depth)?;
		if root != TLS {
			return Err(PackError::Message("expected TLS root".into()));
		}
		if leaf != CHAIN {
			return Err(PackError::Message("expected CHAIN leaf".into()));
		}

		Ok((input, Self {}))
	}
}

/// The private key for [`CertChainKey`], PEM encoded.
#[derive(Debug)]
pub struct CertPrivateKey {}

impl CertPrivateKey {
	pub fn new() -> Self {
		Self {}
	}
}

impl Default for CertPrivateKey {
	fn default() -> Self {
		Self::new()
	}
}

impl FormalKey for CertPrivateKey {
	type Value = Vec<u8>;

	fn deserialize(&self, raw: &[u8]) -> Result<Self::Value> {
		Ok(raw.to_vec())
	}

	fn serialize(&self, value: Self::Value) -> Result<Vec<u8>> {
		Ok(value)
	}
}

impl TuplePack for CertPrivateKey {
	fn pack<W: std::io::Write>(
		&self,
		w: &mut W,
		tuple_depth: TupleDepth,
	) -> std::io::Result<VersionstampOffset> {
		let t = (TLS, KEY);
		t.pack(w, tuple_depth)
	}
}

impl<'de> TupleUnpack<'de> for CertPrivateKey {
	fn unpack(input: &[u8], tuple_depth: TupleDepth) -> PackResult<(&[u8], Self)> {
		let (input, (root, leaf)) = <(usize, usize)>::unpack(input, tuple_depth)?;
		if root != TLS {
			return Err(PackError::Message("expected TLS root".into()));
		}
		if leaf != KEY {
			return Err(PackError::Message("expected KEY leaf".into()));
		}

		Ok((input, Self {}))
	}
}

/// ACME account credentials, in `instant-acme`'s own encoding.
///
/// Persisted for the same reason as the certificate: registering a fresh
/// account on every boot is separately rate limited.
#[derive(Debug)]
pub struct AcmeAccountKey {}

impl AcmeAccountKey {
	pub fn new() -> Self {
		Self {}
	}
}

impl Default for AcmeAccountKey {
	fn default() -> Self {
		Self::new()
	}
}

impl FormalKey for AcmeAccountKey {
	type Value = Vec<u8>;

	fn deserialize(&self, raw: &[u8]) -> Result<Self::Value> {
		Ok(raw.to_vec())
	}

	fn serialize(&self, value: Self::Value) -> Result<Vec<u8>> {
		Ok(value)
	}
}

impl TuplePack for AcmeAccountKey {
	fn pack<W: std::io::Write>(
		&self,
		w: &mut W,
		tuple_depth: TupleDepth,
	) -> std::io::Result<VersionstampOffset> {
		let t = (ACME, ACCOUNT);
		t.pack(w, tuple_depth)
	}
}

impl<'de> TupleUnpack<'de> for AcmeAccountKey {
	fn unpack(input: &[u8], tuple_depth: TupleDepth) -> PackResult<(&[u8], Self)> {
		let (input, (root, leaf)) = <(usize, usize)>::unpack(input, tuple_depth)?;
		if root != ACME {
			return Err(PackError::Message("expected ACME root".into()));
		}
		if leaf != ACCOUNT {
			return Err(PackError::Message("expected ACCOUNT leaf".into()));
		}

		Ok((input, Self {}))
	}
}

/// Held while one instance is ordering, so several do not order at once.
///
/// The value is the timestamp the holder took it. There is no compare-and-set
/// in UniversalDB, so mutual exclusion comes from reading this `Serializable`
/// and writing in the same transaction: the read conflict range makes every
/// loser abort and retry.
#[derive(Debug)]
pub struct AcmeLockKey {}

impl AcmeLockKey {
	pub fn new() -> Self {
		Self {}
	}
}

impl Default for AcmeLockKey {
	fn default() -> Self {
		Self::new()
	}
}

impl FormalKey for AcmeLockKey {
	/// Timestamp, milliseconds.
	type Value = i64;

	fn deserialize(&self, raw: &[u8]) -> Result<Self::Value> {
		Ok(i64::from_be_bytes(raw.try_into()?))
	}

	fn serialize(&self, value: Self::Value) -> Result<Vec<u8>> {
		Ok(value.to_be_bytes().to_vec())
	}
}

impl TuplePack for AcmeLockKey {
	fn pack<W: std::io::Write>(
		&self,
		w: &mut W,
		tuple_depth: TupleDepth,
	) -> std::io::Result<VersionstampOffset> {
		let t = (ACME, LEASE);
		t.pack(w, tuple_depth)
	}
}

impl<'de> TupleUnpack<'de> for AcmeLockKey {
	fn unpack(input: &[u8], tuple_depth: TupleDepth) -> PackResult<(&[u8], Self)> {
		let (input, (root, leaf)) = <(usize, usize)>::unpack(input, tuple_depth)?;
		if root != ACME {
			return Err(PackError::Message("expected ACME root".into()));
		}
		if leaf != LEASE {
			return Err(PackError::Message("expected LEASE leaf".into()));
		}

		Ok((input, Self {}))
	}
}

#[cfg(test)]
#[path = "tls/tests.rs"]
mod tests;
