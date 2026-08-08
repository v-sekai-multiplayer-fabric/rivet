use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::{Result, bail};
use futures_util::StreamExt;
use parking_lot::Mutex;

use crate::{
	driver::TransactionDriver,
	error::DatabaseError,
	key_selector::KeySelector,
	options::{ConflictRangeType, MutationType, Priority, StreamingMode},
	range_option::RangeOption,
	utils::IsolationLevel,
	value::{KeyValue, Slice, Value, Values},
};

use super::error::map_err;

fn is_snapshot(isolation_level: IsolationLevel) -> bool {
	match isolation_level {
		IsolationLevel::Serializable => false,
		IsolationLevel::Snapshot => true,
	}
}

fn streaming_mode(mode: StreamingMode) -> foundationdb::options::StreamingMode {
	use foundationdb::options::StreamingMode as F;

	match mode {
		StreamingMode::WantAll => F::WantAll,
		StreamingMode::Iterator => F::Iterator,
		StreamingMode::Exact => F::Exact,
		StreamingMode::Small => F::Small,
		StreamingMode::Medium => F::Medium,
		StreamingMode::Large => F::Large,
		StreamingMode::Serial => F::Serial,
	}
}

fn mutation_type(op_type: MutationType) -> foundationdb::options::MutationType {
	use foundationdb::options::MutationType as F;

	match op_type {
		MutationType::Add => F::Add,
		MutationType::And => F::And,
		MutationType::BitAnd => F::BitAnd,
		MutationType::Or => F::Or,
		MutationType::BitOr => F::BitOr,
		MutationType::Xor => F::Xor,
		MutationType::BitXor => F::BitXor,
		MutationType::AppendIfFits => F::AppendIfFits,
		MutationType::Max => F::Max,
		MutationType::Min => F::Min,
		MutationType::SetVersionstampedKey => F::SetVersionstampedKey,
		MutationType::SetVersionstampedValue => F::SetVersionstampedValue,
		MutationType::ByteMin => F::ByteMin,
		MutationType::ByteMax => F::ByteMax,
		MutationType::CompareAndClear => F::CompareAndClear,
	}
}

fn conflict_range_type(ty: ConflictRangeType) -> foundationdb::options::ConflictRangeType {
	use foundationdb::options::ConflictRangeType as F;

	match ty {
		ConflictRangeType::Read => F::Read,
		ConflictRangeType::Write => F::Write,
	}
}

fn key_selector(selector: &KeySelector<'_>) -> foundationdb::KeySelector<'static> {
	foundationdb::KeySelector::new(
		selector.key().to_vec().into(),
		selector.or_equal(),
		selector.offset(),
	)
}

fn range_option(opt: &RangeOption<'_>) -> foundationdb::RangeOption<'static> {
	foundationdb::RangeOption {
		begin: key_selector(&opt.begin),
		end: key_selector(&opt.end),
		limit: opt.limit,
		target_bytes: opt.target_bytes,
		mode: streaming_mode(opt.mode),
		reverse: opt.reverse,
		__non_exhaustive: std::marker::PhantomData,
	}
}

/// `commit` and `cancel` consume the underlying transaction, but the trait exposes them behind
/// `&self`, so the handle lives in an `Option` that those calls take from.
pub struct FdbTransactionDriver {
	txn: Mutex<Option<Arc<foundationdb::Transaction>>>,
}

impl FdbTransactionDriver {
	pub fn new(txn: foundationdb::Transaction) -> Self {
		FdbTransactionDriver {
			txn: Mutex::new(Some(Arc::new(txn))),
		}
	}

	fn handle(&self) -> Result<Arc<foundationdb::Transaction>> {
		self.txn
			.lock()
			.clone()
			.ok_or_else(|| DatabaseError::UsedDuringCommit.into())
	}
}

impl TransactionDriver for FdbTransactionDriver {
	fn atomic_op(&self, key: &[u8], param: &[u8], op_type: MutationType) {
		let Ok(txn) = self.handle() else {
			tracing::error!("atomic_op issued after commit");
			return;
		};

		txn.atomic_op(key, param, mutation_type(op_type));
	}

	fn get<'a>(
		&'a self,
		key: &[u8],
		isolation_level: IsolationLevel,
	) -> Pin<Box<dyn Future<Output = Result<Option<Slice>>> + Send + 'a>> {
		let key = key.to_vec();

		Box::pin(async move {
			let txn = self.handle()?;
			let res = txn
				.get(&key, is_snapshot(isolation_level))
				.await
				.map_err(map_err)?;

			Ok(res.map(|slice| Slice::from(slice.to_vec())))
		})
	}

	fn get_key<'a>(
		&'a self,
		selector: &KeySelector<'a>,
		isolation_level: IsolationLevel,
	) -> Pin<Box<dyn Future<Output = Result<Slice>> + Send + 'a>> {
		let selector = key_selector(selector);

		Box::pin(async move {
			let txn = self.handle()?;
			let res = txn
				.get_key(&selector, is_snapshot(isolation_level))
				.await
				.map_err(map_err)?;

			Ok(Slice::from(res.to_vec()))
		})
	}

	fn get_range<'a>(
		&'a self,
		opt: &RangeOption<'a>,
		iteration: usize,
		isolation_level: IsolationLevel,
	) -> Pin<Box<dyn Future<Output = Result<Values>> + Send + 'a>> {
		let opt = range_option(opt);

		Box::pin(async move {
			let txn = self.handle()?;
			let res = txn
				.get_range(&opt, iteration, is_snapshot(isolation_level))
				.await
				.map_err(map_err)?;

			let more = res.more();
			let values = res
				.iter()
				.map(|kv| KeyValue::new(kv.key().to_vec(), kv.value().to_vec()))
				.collect::<Vec<_>>();

			Ok(Values::with_more(values, more))
		})
	}

	fn get_ranges_keyvalues<'a>(
		&'a self,
		opt: RangeOption<'a>,
		isolation_level: IsolationLevel,
	) -> crate::value::Stream<'a, Value> {
		let snapshot = is_snapshot(isolation_level);
		let opt = range_option(&opt);

		let txn = match self.handle() {
			Ok(txn) => txn,
			Err(err) => return Box::pin(futures_util::stream::once(async move { Err(err) })),
		};

		Box::pin(
			futures_util::stream::unfold(
				Some((txn, opt)),
				move |state| async move {
					let (txn, opt) = state?;
					let stream = txn.get_ranges_keyvalues(opt, snapshot);
					let items = stream
						.map(|res| {
							res.map(|kv| Value::new(kv.key().to_vec(), kv.value().to_vec()))
								.map_err(map_err)
						})
						.collect::<Vec<_>>()
						.await;

					Some((futures_util::stream::iter(items), None))
				},
			)
			.flatten(),
		)
	}

	fn set(&self, key: &[u8], value: &[u8]) {
		let Ok(txn) = self.handle() else {
			tracing::error!("set issued after commit");
			return;
		};

		txn.set(key, value);
	}

	fn clear(&self, key: &[u8]) {
		let Ok(txn) = self.handle() else {
			tracing::error!("clear issued after commit");
			return;
		};

		txn.clear(key);
	}

	fn clear_range(&self, begin: &[u8], end: &[u8]) {
		let Ok(txn) = self.handle() else {
			tracing::error!("clear_range issued after commit");
			return;
		};

		txn.clear_range(begin, end);
	}

	fn commit(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
		Box::pin(async move { self.commit_ref().await })
	}

	fn commit_ref(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
		Box::pin(async move {
			let txn = self
				.txn
				.lock()
				.take()
				.ok_or(DatabaseError::UsedDuringCommit)?;

			let txn = match Arc::try_unwrap(txn) {
				Ok(txn) => txn,
				Err(_) => bail!("cannot commit fdb transaction while a range stream is still open"),
			};

			txn.commit().await.map_err(|err| map_err(err.into()))?;

			Ok(())
		})
	}

	fn reset(&mut self) {
		if let Some(txn) = self.txn.lock().as_mut()
			&& let Some(txn) = Arc::get_mut(txn)
		{
			txn.reset();
		}
	}

	fn cancel(&self) {
		let Some(txn) = self.txn.lock().take() else {
			return;
		};

		if let Ok(txn) = Arc::try_unwrap(txn) {
			txn.cancel();
		}
	}

	fn add_conflict_range(
		&self,
		begin: &[u8],
		end: &[u8],
		conflict_type: ConflictRangeType,
	) -> Result<()> {
		let txn = self.handle()?;

		txn.add_conflict_range(begin, end, conflict_range_type(conflict_type))
			.map_err(map_err)
	}

	fn get_estimated_range_size_bytes<'a>(
		&'a self,
		begin: &'a [u8],
		end: &'a [u8],
	) -> Pin<Box<dyn Future<Output = Result<i64>> + Send + 'a>> {
		Box::pin(async move {
			let txn = self.handle()?;

			txn.get_estimated_range_size_bytes(begin, end)
				.await
				.map_err(map_err)
		})
	}

	fn tag(&self, tag: &str) -> Result<()> {
		let txn = self.handle()?;

		txn.set_option(foundationdb::options::TransactionOption::Tag(
			tag.to_string(),
		))
		.map_err(map_err)
	}

	fn priority(&self, priority: Priority) -> Result<()> {
		let txn = self.handle()?;

		let opt = match priority {
			Priority::Low => foundationdb::options::TransactionOption::PriorityBatch,
			Priority::Default => return Ok(()),
			Priority::High => foundationdb::options::TransactionOption::PrioritySystemImmediate,
		};

		txn.set_option(opt).map_err(map_err)
	}
}
