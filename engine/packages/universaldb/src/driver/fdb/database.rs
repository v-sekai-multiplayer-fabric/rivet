use std::{
	path::PathBuf,
	sync::{
		Arc, OnceLock,
		atomic::{AtomicI32, Ordering},
	},
};

use anyhow::{Context, Result};
use foundationdb::api::NetworkAutoStop;

use crate::{
	RetryableTransaction, Transaction,
	driver::{BoxFut, DatabaseDriver, Erased},
	error::DatabaseError,
	transaction::TXN_TIMEOUT,
	utils::{MaybeCommitted, calculate_tx_retry_backoff},
};

use super::{error::FdbDriverError, transaction::FdbTransactionDriver};

/// The FDB network runs on a dedicated thread that can only be started once per process. Held in a
/// static so it outlives every `Database` handle. Statics are never dropped, which is what we want:
/// stopping the network while a handle is still open aborts the process.
static NETWORK: OnceLock<NetworkAutoStop> = OnceLock::new();

fn boot_network() {
	NETWORK.get_or_init(|| unsafe { foundationdb::boot() });
}

#[derive(Clone, Debug, Default)]
pub struct FdbConfig {
	pub cluster_file: Option<PathBuf>,
}

pub struct FdbDatabaseDriver {
	db: Arc<foundationdb::Database>,
	max_retries: AtomicI32,
}

impl FdbDatabaseDriver {
	pub async fn new(config: FdbConfig) -> Result<Self> {
		tracing::debug!(cluster_file = ?config.cluster_file, "creating FdbDatabaseDriver");

		boot_network();

		let db = match &config.cluster_file {
			Some(path) => {
				let path = path
					.to_str()
					.context("cluster file path is not valid UTF-8")?;
				foundationdb::Database::from_path(path)
					.with_context(|| format!("failed to open fdb cluster file {path}"))?
			}
			None => foundationdb::Database::default()
				.context("failed to open default fdb cluster file")?,
		};

		Ok(FdbDatabaseDriver {
			db: Arc::new(db),
			max_retries: AtomicI32::new(100),
		})
	}
}

impl DatabaseDriver for FdbDatabaseDriver {
	fn create_txn(&self) -> Result<Transaction> {
		let txn = self
			.db
			.create_trx()
			.map_err(super::error::map_err)
			.context("failed to create fdb transaction")?;

		Ok(Transaction::new(Arc::new(FdbTransactionDriver::new(txn))))
	}

	fn run<'a>(
		&'a self,
		closure: Box<dyn Fn(RetryableTransaction) -> BoxFut<'a, Result<Erased>> + Send + Sync + 'a>,
	) -> BoxFut<'a, Result<Erased>> {
		Box::pin(async move {
			let mut maybe_committed = MaybeCommitted(false);
			let max_retries = self.max_retries.load(Ordering::SeqCst);

			for attempt in 0..max_retries {
				let tx = self.create_txn()?;
				let mut retryable = RetryableTransaction::new(tx);
				retryable.maybe_committed = maybe_committed;

				let error =
					match tokio::time::timeout(TXN_TIMEOUT, closure(retryable.clone())).await {
						Ok(Ok(res)) => match retryable.inner.driver.commit_ref().await {
							Ok(_) => return Ok(res),
							Err(e) => e,
						},
						Ok(Err(e)) => e,
						Err(_) => anyhow::Error::from(DatabaseError::TransactionTooOld),
					};

				// Resolve the retry verdict before awaiting so no `dyn Error` borrow, which is not
				// `Sync`, is held across the backoff. FDB classifies its own errors, so prefer its
				// verdict over the generic mapping.
				let retry = {
					let fdb_error = error
						.chain()
						.find_map(|x| x.downcast_ref::<FdbDriverError>());

					match fdb_error {
						Some(fdb_error) => fdb_error
							.is_retryable()
							.then(|| MaybeCommitted(fdb_error.is_maybe_committed())),
						None => error
							.chain()
							.find_map(|x| x.downcast_ref::<DatabaseError>())
							.filter(|db_error| db_error.is_retryable())
							.map(|db_error| MaybeCommitted(db_error.is_maybe_committed())),
					}
				};

				let Some(next_maybe_committed) = retry else {
					return Err(error);
				};

				if *next_maybe_committed {
					maybe_committed = next_maybe_committed;
				}

				let backoff_ms = calculate_tx_retry_backoff(attempt as usize);
				tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
			}

			Err(DatabaseError::MaxRetriesReached.into())
		})
	}

	fn txn_retry_limit(&self, limit: i32) -> Result<()> {
		self.max_retries.store(limit, Ordering::SeqCst);
		Ok(())
	}
}
