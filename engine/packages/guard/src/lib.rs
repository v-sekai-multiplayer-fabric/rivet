use anyhow::*;
use gas::prelude::*;

pub mod cache;
pub mod errors;
pub mod metrics;
pub mod routing;
pub mod shared_state;
pub mod tls;

#[tracing::instrument(skip_all)]
pub async fn start(config: rivet_config::Config, pools: rivet_pools::Pools) -> Result<()> {
	let cache = rivet_cache::CacheInner::from_env(&config, pools.clone())?;
	let ctx = StandaloneCtx::new(
		db::DatabaseKv::new(config.clone(), pools.clone()).await?,
		config.clone(),
		pools,
		cache,
		"guard",
		Id::new_v1(config.dc_label()),
		Id::new_v1(config.dc_label()),
	)?;

	// Share shared context
	let shared_state = shared_state::SharedState::new(&config, ctx.ups()?);
	shared_state.start().await?;

	// Create handlers
	let routing_fn = routing::create_routing_function(&ctx, shared_state.clone());
	let cache_key_fn = cache::create_cache_key_function();
	// The slot is kept alongside the resolver so certificate issuance can swap a
	// certificate in after the listeners are already bound.
	let (cert_resolver, _cert_slot) = match tls::create_cert_resolver(&ctx).await? {
		Some((resolver, slot)) => {
			tracing::info!("TLS certificate resolver configured");
			(Some(resolver), Some(slot))
		}
		None => {
			tracing::info!("No TLS configuration found, HTTPS will not be enabled");
			(None, None)
		}
	};

	// Start the server
	tracing::info!("starting proxy server");
	rivet_guard_core::run_server(config, routing_fn, cache_key_fn, cert_resolver).await
}
