//! Exercises the FoundationDB driver against a real cluster.
//!
//! Requires `libfdb_c.so` and a reachable cluster. Set `FDB_CLUSTER_FILE` to run; the test is
//! skipped otherwise so the suite still passes on machines without FDB.
#![cfg(feature = "foundationdb")]

use std::sync::Arc;

use futures_util::TryStreamExt;
use universaldb::{
	Database,
	driver::fdb::{FdbConfig, FdbDatabaseDriver},
	options::{ConflictRangeType, MutationType, StreamingMode},
	range_option::RangeOption,
	utils::{IsolationLevel, end_of_key_range},
};
use uuid::Uuid;

async fn db() -> Option<Database> {
	let cluster_file = std::env::var("FDB_CLUSTER_FILE").ok()?;

	let driver = FdbDatabaseDriver::new(FdbConfig {
		cluster_file: Some(cluster_file.into()),
	})
	.await
	.expect("failed to open fdb");

	Some(Database::new(Arc::new(driver)))
}

/// Namespaces each test run so repeated runs against a persistent cluster stay independent.
fn prefix() -> Vec<u8> {
	format!("udb-test/{}/", Uuid::new_v4()).into_bytes()
}

fn key(prefix: &[u8], suffix: &str) -> Vec<u8> {
	let mut key = prefix.to_vec();
	key.extend_from_slice(suffix.as_bytes());
	key
}

#[tokio::test]
async fn set_and_get_roundtrip() {
	let Some(db) = db().await else { return };
	let prefix = prefix();

	let k = key(&prefix, "alpha");
	let write_key = k.clone();
	db.txn("set", move |tx| {
		let write_key = write_key.clone();
		async move {
			tx.set(&write_key, b"beta");
			Ok(())
		}
	})
	.await
	.unwrap();

	let read_key = k.clone();
	let value = db
		.txn("get", move |tx| {
			let read_key = read_key.clone();
			async move { tx.get(&read_key, IsolationLevel::Serializable).await }
		})
		.await
		.unwrap();

	assert_eq!(value.as_deref().map(|v| v.to_vec()), Some(b"beta".to_vec()));
}

#[tokio::test]
async fn missing_key_reads_none() {
	let Some(db) = db().await else { return };
	let prefix = prefix();

	let k = key(&prefix, "absent");
	let value = db
		.txn("get_missing", move |tx| {
			let k = k.clone();
			async move { tx.get(&k, IsolationLevel::Serializable).await }
		})
		.await
		.unwrap();

	assert!(value.is_none());
}

#[tokio::test]
async fn range_scan_returns_sorted_keys() {
	let Some(db) = db().await else { return };
	let prefix = prefix();

	let write_prefix = prefix.clone();
	db.txn("seed", move |tx| {
		let write_prefix = write_prefix.clone();
		async move {
			for i in 0..10u32 {
				tx.set(&key(&write_prefix, &format!("k{i:03}")), &i.to_be_bytes());
			}
			Ok(())
		}
	})
	.await
	.unwrap();

	let scan_prefix = prefix.clone();
	let keys = db
		.txn("scan", move |tx| {
			let scan_prefix = scan_prefix.clone();
			async move {
				let opt = RangeOption {
					mode: StreamingMode::WantAll,
					..RangeOption::from((
						scan_prefix.clone(),
						end_of_key_range(&key(&scan_prefix, "k999")),
					))
				};

				tx.get_ranges_keyvalues(opt, IsolationLevel::Serializable)
					.try_collect::<Vec<_>>()
					.await
			}
		})
		.await
		.unwrap();

	assert_eq!(keys.len(), 10);
	let decoded = keys
		.iter()
		.map(|v| u32::from_be_bytes(v.value().try_into().unwrap()))
		.collect::<Vec<_>>();
	assert_eq!(decoded, (0..10).collect::<Vec<_>>());
}

#[tokio::test]
async fn atomic_add_accumulates() {
	let Some(db) = db().await else { return };
	let prefix = prefix();
	let k = key(&prefix, "counter");

	for _ in 0..5 {
		let k = k.clone();
		db.txn("add", move |tx| {
			let k = k.clone();
			async move {
				tx.informal().atomic_op(&k, &3u64.to_le_bytes(), MutationType::Add);
				Ok(())
			}
		})
		.await
		.unwrap();
	}

	let read_key = k.clone();
	let value = db
		.txn("read_counter", move |tx| {
			let read_key = read_key.clone();
			async move {
				tx.get(&read_key, IsolationLevel::Serializable)
					.await
			}
		})
		.await
		.unwrap()
		.expect("counter missing");

	assert_eq!(u64::from_le_bytes(value.as_slice().try_into().unwrap()), 15);
}

#[tokio::test]
async fn clear_removes_key() {
	let Some(db) = db().await else { return };
	let prefix = prefix();
	let k = key(&prefix, "doomed");

	let write_key = k.clone();
	db.txn("set_then_clear", move |tx| {
		let write_key = write_key.clone();
		async move {
			tx.set(&write_key, b"x");
			Ok(())
		}
	})
	.await
	.unwrap();

	let clear_key = k.clone();
	db.txn("clear", move |tx| {
		let clear_key = clear_key.clone();
		async move {
			tx.clear(&clear_key);
			Ok(())
		}
	})
	.await
	.unwrap();

	let read_key = k.clone();
	let value = db
		.txn("read_cleared", move |tx| {
			let read_key = read_key.clone();
			async move { tx.get(&read_key, IsolationLevel::Serializable).await }
		})
		.await
		.unwrap();

	assert!(value.is_none());
}

#[tokio::test]
async fn add_conflict_range_is_accepted() {
	let Some(db) = db().await else { return };
	let prefix = prefix();
	let k = key(&prefix, "conflict");

	let conflict_key = k.clone();
	db.txn("conflict", move |tx| {
		let conflict_key = conflict_key.clone();
		async move {
			tx.add_conflict_range(
				&conflict_key,
				&end_of_key_range(&conflict_key),
				ConflictRangeType::Read,
			)?;
			tx.set(&conflict_key, b"v");
			Ok(())
		}
	})
	.await
	.unwrap();
}
