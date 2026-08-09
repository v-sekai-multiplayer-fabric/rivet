//! Regression test for the serverless runner-pool jam.
//!
//! The mk1 serverless pool decides how many runners to start from a stored
//! `ServerlessDesiredSlotsKey` counter that is incremented on allocate and
//! decremented on destroy. Those two sites are gated on different predicates, so
//! under churn the counter drifts negative and the pool clamps the desired
//! runner count to zero. Because nothing lifts a negative counter back up, the
//! pool then refuses to start a runner even while real actors are queued and
//! waiting: a permanent jam (machine-checked in `serverless_pool_jam.lean` as
//! `counter_can_jam` / `jam_is_sink`).
//!
//! This test constructs that jammed state directly: it seeds the counter
//! negative (as accumulated drift would) and enqueues one real pending actor,
//! then asserts the pool still starts a runner to serve it. On the counter-based
//! `read_desired` this fails (the mock `/start` is never called). After the
//! reconciler change, `read_desired` derives demand from the pending queue, so a
//! queued actor always yields a positive desired count and the pool recovers.

#[path = "common/api/mod.rs"]
mod api;
#[path = "common/ctx.rs"]
mod ctx;

use axum::{
	Router,
	body::Bytes,
	extract::State,
	http::StatusCode,
	response::{
		IntoResponse, Response, Sse,
		sse::{Event, KeepAlive},
	},
	routing::get,
};
use futures_util::stream;
use rivet_types::keys;
use serde_json::json;
use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

struct MockServerlessState {
	start_tx: mpsc::UnboundedSender<()>,
}

/// mk1 metadata: no `envoyProtocolVersion`, so the pool counter drives scaling
/// rather than the per-actor envoy start path.
async fn metadata_handler() -> Response {
	(
		StatusCode::OK,
		[(axum::http::header::CONTENT_TYPE, "application/json")],
		json!({ "runtime": "rivetkit", "version": "1" }).to_string(),
	)
		.into_response()
}

async fn start_handler(
	State(state): State<Arc<MockServerlessState>>,
	_body: Bytes,
) -> impl IntoResponse {
	let _ = state.start_tx.send(());
	let events =
		stream::once(async { Ok::<Event, Infallible>(Event::default().event("ping").data("")) });
	Sse::new(events)
		.keep_alive(KeepAlive::default())
		.into_response()
}

fn run<F, Fut>(opts: ctx::TestOpts, test_fn: F)
where
	F: FnOnce(ctx::TestCtx) -> Fut,
	Fut: Future<Output = ()>,
{
	let runtime = tokio::runtime::Runtime::new().expect("failed to build runtime");
	runtime.block_on(async {
		let timeout = Duration::from_secs(opts.timeout_secs);
		let ctx = ctx::TestCtx::new_with_opts(opts)
			.await
			.expect("failed to build test ctx");
		tokio::time::timeout(timeout, test_fn(ctx))
			.await
			.expect("test timed out");
	});
}

async fn setup_test_namespace(leader_dc: &ctx::TestDatacenter) -> (String, rivet_util::Id) {
	let random_suffix = rand::random::<u16>();
	let namespace_name = format!("test-{random_suffix}");
	let response = api::public::namespaces_create(
		leader_dc.guard_port(),
		rivet_api_peer::namespaces::CreateRequest {
			name: namespace_name,
			display_name: "Test Namespace".to_string(),
		},
	)
	.await
	.expect("failed to set up test namespace");

	(response.namespace.name, response.namespace.namespace_id)
}

#[test]
fn serverless_pool_serves_demand_despite_negative_counter_drift() {
	run(
		ctx::TestOpts::new(1)
			.with_timeout(30)
			.with_pegboard_outbound(),
		|ctx| async move {
			let (namespace, namespace_id) = setup_test_namespace(ctx.leader_dc()).await;
			let runner_name = "serverless-pool-reconcile";

			let (start_tx, mut start_rx) = mpsc::unbounded_channel();
			let mock_state = Arc::new(MockServerlessState { start_tx });
			// The mk1 serverless conn opens the runner with GET /start (an SSE
			// stream), see pegboard serverless conn.rs.
			let app = Router::new()
				.route("/metadata", get(metadata_handler))
				.route("/start", get(start_handler))
				.with_state(mock_state.clone());

			let mock_port = portpicker::pick_unused_port().expect("failed to pick port");
			let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{mock_port}"))
				.await
				.expect("failed to bind mock serverless endpoint");
			let server_handle = tokio::spawn(async move {
				axum::serve(listener, app).await.expect("server error");
			});

			// mk1 serverless pool, scaled purely by demand: no floor from
			// min_runners or margin, room for one runner.
			let mut datacenters = HashMap::new();
			datacenters.insert(
				"dc-1".to_string(),
				rivet_api_types::namespaces::runner_configs::RunnerConfig {
					kind:
						rivet_api_types::namespaces::runner_configs::RunnerConfigKind::Serverless {
							url: format!("http://127.0.0.1:{mock_port}"),
							headers: None,
							request_lifespan: 30,
							max_concurrent_actors: Some(10),
							drain_grace_period: Some(5),
							slots_per_runner: Some(1),
							min_runners: Some(0),
							max_runners: Some(1),
							runners_margin: Some(0),
							metadata_poll_interval: None,
							drain_on_version_upgrade: None,
							actor_eviction_delay: None,
							actor_eviction_period: None,
							actor_eviction_rate: None,
						},
					metadata: None,
					drain_on_version_upgrade: None,
				},
			);

			api::public::runner_configs_upsert(
				ctx.leader_dc().guard_port(),
				rivet_api_peer::runner_configs::UpsertPath {
					runner_name: runner_name.to_string(),
				},
				rivet_api_peer::runner_configs::UpsertQuery {
					namespace: namespace.clone(),
				},
				rivet_api_public::runner_configs::upsert::UpsertRequest { datacenters },
			)
			.await
			.expect("failed to upsert serverless runner config");

			// Put one real actor in the pending queue, exactly as a failed
			// allocation would (PendingActorByRunnerNameSelectorKey is the
			// authoritative record of an actor waiting for a slot), while
			// leaving the desired-slots counter at zero. This is the desynced
			// state churn produces: a queued actor the counter has lost track
			// of. It is not negative, so the "heal negative to zero" band-aid
			// does nothing here; only reconciling from the queue can recover.
			let pending_actor_id = rivet_util::Id::new_v1(1);
			ctx.leader_dc()
				.workflow_ctx
				.udb()
				.expect("udb")
				.txn("inject_pending_serverless_demand", |tx| {
					let runner_name = runner_name.to_string();
					async move {
						let tx = tx.with_subspace(keys::pegboard::subspace());
						tx.write(
							&pegboard::keys::ns::PendingActorByRunnerNameSelectorKey::new(
								namespace_id,
								runner_name,
								rivet_util::timestamp::now(),
								pending_actor_id,
							),
							0u32,
						)?;
						Ok(())
					}
				})
				.await
				.expect("failed to inject pending demand");

			// Wake the pool so it re-evaluates desired runners.
			ctx.leader_dc()
				.workflow_ctx
				.signal(pegboard::workflows::runner_pool::Bump::default())
				.to_workflow::<pegboard::workflows::runner_pool::Workflow>()
				.tag("namespace_id", namespace_id)
				.tag("runner_name", runner_name)
				.send()
				.await
				.expect("failed to bump runner pool");

			// The pool must start a runner to serve the queued actor. On the
			// counter-based read_desired the zero counter yields zero desired
			// and this times out; the reconciler counts the pending queue and
			// starts a runner.
			tokio::time::timeout(Duration::from_secs(15), start_rx.recv())
				.await
				.expect("serverless pool never started a runner for a queued actor (jammed)")
				.expect("mock serverless start channel closed");

			server_handle.abort();
		},
	);
}
