//! Certificate loading for Guard's TLS listeners.
//!
//! The certificate lives behind a swappable slot rather than being loaded once
//! and captured. That is what lets HTTP-01 work: ACME cannot complete until the
//! plain HTTP listener is answering challenges, but the QUIC listener is only
//! spawned when a resolver already exists (`guard-core/src/server.rs`). So the
//! resolver is handed over immediately with an empty slot, both listeners bind,
//! and the certificate is swapped in when it arrives.
//!
//! A handshake attempted before then fails rather than serving something wrong.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use anyhow::*;
use gas::prelude::*;
use parking_lot::RwLock;
use rivet_guard_core::CertResolverFn;
use rustls::crypto::ring::sign::any_supported_type;
use rustls::sign::CertifiedKey;

/// The certificate Guard is currently serving, if any.
///
/// `parking_lot` rather than `tokio::sync` because rustls calls the resolver
/// synchronously from inside the handshake, which is a forced-sync context.
#[derive(Clone, Default)]
pub struct CertSlot {
	inner: Arc<RwLock<Option<Arc<CertifiedKey>>>>,
}

impl CertSlot {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn get(&self) -> Option<Arc<CertifiedKey>> {
		self.inner.read().clone()
	}

	pub fn set(&self, cert: Arc<CertifiedKey>) {
		*self.inner.write() = Some(cert);
	}

	/// Load a PEM certificate chain and private key into the slot.
	pub fn load_from_paths(&self, cert_path: &Path, key_path: &Path) -> Result<()> {
		let cert = load_certified_key(cert_path, key_path)?;
		self.set(cert);

		tracing::info!(
			cert_path = %cert_path.display(),
			"loaded TLS certificate"
		);

		Ok(())
	}
}

/// Read a PEM chain and key from disk into a rustls `CertifiedKey`.
pub fn load_certified_key(cert_path: &Path, key_path: &Path) -> Result<Arc<CertifiedKey>> {
	let cert_file = File::open(cert_path)
		.with_context(|| format!("failed to open certificate {}", cert_path.display()))?;
	let cert_chain = rustls_pemfile::certs(&mut BufReader::new(cert_file))
		.collect::<std::result::Result<Vec<_>, _>>()
		.with_context(|| format!("failed to parse certificate {}", cert_path.display()))?;

	ensure!(
		!cert_chain.is_empty(),
		"certificate {} contains no certificates",
		cert_path.display()
	);

	let key_file = File::open(key_path)
		.with_context(|| format!("failed to open private key {}", key_path.display()))?;
	let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))
		.with_context(|| format!("failed to parse private key {}", key_path.display()))?
		.with_context(|| format!("private key {} contains no key", key_path.display()))?;

	let signing_key = any_supported_type(&key)
		.with_context(|| format!("unsupported private key type in {}", key_path.display()))?;

	Ok(Arc::new(CertifiedKey::new(cert_chain, signing_key)))
}

/// Build the resolver Guard serves TLS from.
///
/// Returns `None` only when HTTPS is not configured at all, because that is the
/// signal `guard-core` uses to skip both the TLS acceptor and the QUIC
/// listener. A configured-but-not-yet-issued certificate still yields a
/// resolver, so the listeners bind and ACME has somewhere to deliver to.
pub async fn create_cert_resolver(
	ctx: &gas::prelude::StandaloneCtx,
) -> Result<Option<(CertResolverFn, CertSlot)>> {
	let Some(https) = &ctx.config().guard().https else {
		tracing::info!("no HTTPS configuration, TLS and HTTP/3 disabled");
		return Ok(None);
	};

	let slot = CertSlot::new();

	// A certificate already on disk is served immediately. This is the path a
	// local run takes, and the path a restart takes once ACME has written one.
	//
	// Only the API pair is read: one certificate covers both surfaces in a
	// single-host deployment, and the actor/API split existed to serve
	// different hostnames from datacenter config that no longer exists.
	let cert_path = &https.tls.api_cert_path;
	let key_path = &https.tls.api_key_path;

	if cert_path.exists() && key_path.exists() {
		// A certificate that is present but unreadable is a configuration
		// error, not a reason to start without TLS.
		slot.load_from_paths(cert_path, key_path)
			.context("failed to load the configured TLS certificate")?;
	} else {
		tracing::info!(
			cert_path = %cert_path.display(),
			"no certificate on disk yet, TLS handshakes will fail until one is issued"
		);
	}

	let resolver_slot = slot.clone();
	let resolver: CertResolverFn = Arc::new(move |server_name: &str| {
		resolver_slot.get().ok_or_else(|| {
			let err: Box<dyn std::error::Error + Send + Sync> =
				format!("no certificate available yet for {server_name}").into();
			err
		})
	});

	Ok(Some((resolver, slot)))
}

#[cfg(test)]
#[path = "tls/tests.rs"]
mod tests;
