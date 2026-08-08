//! Durable state Guard owns.
//!
//! Only TLS material lives here so far, and it lives here rather than on disk
//! for one reason: the certificate must survive a restart. A Fly machine has no
//! volume, and Let's Encrypt permits five duplicate certificates per name set
//! per week across all accounts, so a certificate reissued on every boot would
//! exhaust that within an afternoon of deploys and lock the service out for a
//! week.
//!
//! Guard takes its own subspace rather than borrowing pegboard's, whose root is
//! scanned and cleared wholesale by orchestration GC.

use universaldb::prelude::*;

pub mod tls;

pub use self::tls::{AcmeAccountKey, AcmeLockKey, CertChainKey, CertPrivateKey};

pub fn subspace() -> universaldb::utils::Subspace {
	universaldb::utils::Subspace::new(&(RIVET, GUARD))
}
