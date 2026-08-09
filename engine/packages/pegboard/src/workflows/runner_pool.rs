use std::hash::{DefaultHasher, Hash, Hasher};

use futures_util::{FutureExt, TryStreamExt};
use gas::prelude::*;
use rivet_types::runner_configs::RunnerConfigKind;
use universaldb::prelude::*;

use super::{runner_pool_error_tracker, runner_pool_metadata_poller, serverless};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Input {
	pub namespace_id: Id,
	pub runner_name: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct LifecycleState {
	runners: Vec<RunnerState>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RunnerState {
	receiver_wf_id: Id,
	details_hash: u64,
}

#[workflow]
pub async fn pegboard_runner_pool2(ctx: &mut WorkflowCtx, input: &Input) -> Result<()> {
	// Exit before starting sidecar workflows if there is no serverless runner config to manage.
	if matches!(
		ctx.v(4)
			.activity(ReadDesiredInput {
				namespace_id: input.namespace_id,
				runner_name: input.runner_name.clone(),
			})
			.await?,
		ReadDesiredOutput::Stop
	) {
		return Ok(());
	}

	let error_tracker_wf_id = ctx
		.v(2)
		.workflow(runner_pool_error_tracker::Input {
			namespace_id: input.namespace_id,
			runner_name: input.runner_name.clone(),
		})
		.tag("namespace_id", input.namespace_id)
		.tag("runner_name", &input.runner_name)
		.unique()
		.dispatch()
		.await?;

	ctx.v(3)
		.workflow(runner_pool_metadata_poller::Input {
			namespace_id: input.namespace_id,
			runner_name: input.runner_name.clone(),
		})
		.tag("namespace_id", input.namespace_id)
		.tag("runner_name", &input.runner_name)
		.unique()
		.dispatch()
		.await?;

	ctx.lupe()
		.commit_interval(5)
		.with_state(LifecycleState::default())
		.run(|ctx, state| {
			let input = input.clone();
			async move {
				// Get desired count -> drain and start counts
				let ReadDesiredOutput::Desired {
					desired_count,
					details_hash,
				} = ctx.activity(ReadDesiredInput {
					namespace_id: input.namespace_id,
					runner_name: input.runner_name.clone(),
				})
				.await?
				else {
					// Drain all
					for runner in &state.runners {
						ctx.signal(serverless::receiver::Drain {})
							.to_workflow_id(runner.receiver_wf_id)
							.send()
							.await?;
					}

					return Ok(Loop::Break(()));
				};

				// Remove runners that have an outdated hash. This is done outside of the below draining mechanism
				// because we drain specific runners, not just a number of runners
				let (new, outdated) = std::mem::take(&mut state.runners)
					.into_iter()
					.partition::<Vec<_>, _>(|r| r.details_hash == details_hash);
				state.runners = new;

				for runner in outdated {
					// TODO: Spawn sub wf to process these so this is not blocking the loop
					ctx.signal(serverless::receiver::Drain {})
						.to_workflow_id(runner.receiver_wf_id)
						.send()
						.await?;
				}

				// These will never both be non-zero
				let drain_count = state.runners.len().saturating_sub(desired_count);
				let start_count = desired_count.saturating_sub(state.runners.len());

				// Drain unnecessary runners
				if drain_count != 0 {
					// TODO: Implement smart logic of draining runners with the lowest allocated actors
					let remaining_runners = state.runners.split_off(drain_count);
					let draining_runners = std::mem::replace(&mut state.runners, remaining_runners);

					// TODO: Spawn sub wf to process these so this is not blocking the loop
					for runner in draining_runners {
						ctx.signal(serverless::receiver::Drain {})
							.to_workflow_id(runner.receiver_wf_id)
							.send()
							.await?;
					}
				}
				// Dispatch new runner workflows
				else if start_count != 0 {
					// TODO: Spawn sub wf to process these so this is not blocking the loop
					for _ in 0..start_count {
						let receiver_wf_id = ctx
							.workflow(serverless::receiver::Input {
								pool_wf_id: ctx.workflow_id(),
								namespace_id: input.namespace_id,
								runner_name: input.runner_name.clone(),
							})
							.tag("namespace_id", input.namespace_id)
							.tag("runner_name", input.runner_name.clone())
							.dispatch()
							.await?;

						state.runners.push(RunnerState {
							receiver_wf_id,
							details_hash,
						});
					}
				}

				// Wait for Bump or serverless signals until we tick again
				for sig in ctx.listen_n::<Main>(256).await? {
					match sig {
						Main::OutboundConnDrainStarted(sig) => {
							let (new, drain_started) = std::mem::take(&mut state.runners)
								.into_iter()
								.partition::<Vec<_>, _>(|r| r.receiver_wf_id != sig.receiver_wf_id);
							state.runners = new;

							for runner in drain_started {
								// TODO: Spawn sub wf to process these so this is not blocking the loop
								ctx.signal(serverless::receiver::Drain {})
									.to_workflow_id(runner.receiver_wf_id)
									.send()
									.await?;
							}
						}
						Main::Bump(bump) => {
							if bump.endpoint_config_changed {
								// Forward to metadata poller to trigger immediate metadata fetch
								ctx.signal(runner_pool_metadata_poller::EndpointConfigChanged {})
									.to_workflow::<runner_pool_metadata_poller::Workflow>()
									.tag("namespace_id", input.namespace_id)
									.tag("runner_name", &input.runner_name)
									.send()
									.await?;
							}
						}
					}
				}

				Ok(Loop::Continue)
			}
			.boxed()
		})
		.await?;

	ctx.v(2)
		.signal(crate::workflows::runner_pool_error_tracker::Shutdown {})
		.to_workflow_id(error_tracker_wf_id)
		.send()
		.await?;

	Ok(())
}

#[derive(Debug, Serialize, Deserialize, Hash)]
struct ReadDesiredInput {
	namespace_id: Id,
	runner_name: String,
}

// Should have `#[serde(rename_all = "snake_case")]` but doesn't
#[derive(Debug, Serialize, Deserialize)]
enum ReadDesiredOutput {
	Desired {
		desired_count: usize,
		details_hash: u64,
	},
	Stop,
}

#[activity(ReadDesired)]
async fn read_desired(ctx: &ActivityCtx, input: &ReadDesiredInput) -> Result<ReadDesiredOutput> {
	let udb_pool = ctx.udb()?;
	let (runner_config_res, demand_slots) = tokio::try_join!(
		ctx.op(crate::ops::runner_config::get::Input {
			runners: vec![(input.namespace_id, input.runner_name.clone())],
			bypass_cache: false,
		}),
		udb_pool.txn("pegboard_runner_pool_read_demand", |tx| async move {
			count_demand_slots(&tx, input.namespace_id, &input.runner_name).await
		}),
	)?;
	let Some(runner_config) = runner_config_res.into_iter().next() else {
		return Ok(ReadDesiredOutput::Stop);
	};

	let RunnerConfigKind::Serverless {
		url,
		headers,

		slots_per_runner,
		min_runners,
		max_runners,
		runners_margin,
		..
	} = runner_config.config.kind
	else {
		return Ok(ReadDesiredOutput::Stop);
	};

	// Envoys don't need this workflow
	if runner_config.protocol_version.is_some() {
		return Ok(ReadDesiredOutput::Desired {
			desired_count: 0,
			details_hash: 0,
		});
	}

	// Level-triggered reconciliation: desired runners is derived every tick from
	// the observed live demand (`count_demand_slots`), never from an accumulated
	// counter. Because demand is recomputed from the source-of-truth indexes it
	// cannot drift below reality, so the pool cannot wedge at zero while actors
	// wait. See serverless_pool_jam.lean (`reconciler_never_jams`).
	let demand_slots = u32::try_from(demand_slots).unwrap_or(u32::MAX);

	// Won't overflow as these values are all in u32 range
	let desired_count = (runners_margin + demand_slots.div_ceil(slots_per_runner.max(1)))
		.max(min_runners)
		.min(max_runners)
		.min(
			ctx.config()
				.pegboard()
				.pool_desired_max_override
				.unwrap_or(u32::MAX),
		)
		.try_into()?;

	// Compute consistent hash of serverless details
	let mut hasher = DefaultHasher::new();
	url.hash(&mut hasher);
	let mut sorted_headers = headers.iter().collect::<Vec<_>>();
	sorted_headers.sort();
	sorted_headers.hash(&mut hasher);
	let details_hash = hasher.finish();

	Ok(ReadDesiredOutput::Desired {
		desired_count,
		details_hash,
	})
}

/// Counts the live serverless demand for a runner pool, in slots: actors waiting
/// in the pending queue plus actors already running on the pool's runners
/// (counted as used slots). This is the level-triggered source of truth that
/// replaces the drift-prone `ServerlessDesiredSlotsKey` counter. Overcounting is
/// safe (an extra runner drains on the next tick); undercounting would drain a
/// runner hosting a live actor, so transient pending/running overlap biases the
/// count upward by design.
async fn count_demand_slots(
	tx: &universaldb::Transaction,
	namespace_id: Id,
	runner_name: &str,
) -> Result<u64> {
	let tx = tx.with_subspace(crate::keys::subspace());

	// Actors waiting for a slot.
	let pending_subspace = crate::keys::subspace().subspace(
		&crate::keys::ns::PendingActorByRunnerNameSelectorKey::subspace(
			namespace_id,
			runner_name.to_string(),
		),
	);
	let mut pending = 0u64;
	let mut pending_stream = tx.get_ranges_keyvalues(
		universaldb::RangeOption {
			mode: StreamingMode::WantAll,
			..(&pending_subspace).into()
		},
		Snapshot,
	);
	while pending_stream.try_next().await?.is_some() {
		pending += 1;
	}

	// Actors already allocated on this pool's runners, counted as used slots.
	let alloc_subspace = crate::keys::subspace().subspace(
		&crate::keys::ns::RunnerAllocIdxKey::subspace(namespace_id, runner_name.to_string()),
	);
	let mut running = 0u64;
	let mut alloc_stream = tx.get_ranges_keyvalues(
		universaldb::RangeOption {
			mode: StreamingMode::WantAll,
			..(&alloc_subspace).into()
		},
		Snapshot,
	);
	while let Some(entry) = alloc_stream.try_next().await? {
		let (_, data) = tx.read_entry::<crate::keys::ns::RunnerAllocIdxKey>(&entry)?;
		running += u64::from(data.total_slots.saturating_sub(data.remaining_slots));
	}

	Ok(pending + running)
}

#[signal("pegboard_runner_pool_bump")]
#[derive(Debug, Default)]
pub struct Bump {
	#[serde(default)]
	pub endpoint_config_changed: bool,
}

#[signal("pegboard_outbound_conn_drain_started")]
pub struct OutboundConnDrainStarted {
	pub receiver_wf_id: Id,
}

join_signal!(Main {
	Bump,
	OutboundConnDrainStarted,
});
