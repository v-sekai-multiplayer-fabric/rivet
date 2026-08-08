use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod postgres;

pub use postgres::{Postgres, PostgresSsl};

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Database {
	Postgres(Postgres),
	FileSystem(FileSystem),
	#[serde(rename = "foundationdb")]
	FoundationDb(FoundationDb),
}

impl Default for Database {
	fn default() -> Self {
		Self::FileSystem(FileSystem::default())
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileSystem {
	pub path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FoundationDb {
	/// Path to an existing `fdb.cluster` file. Takes precedence over `addresses`.
	pub cluster_file: Option<PathBuf>,

	/// Coordinator addresses. When set without `cluster_file`, a cluster file is written to
	/// `cluster_file_write_path` on startup. IPv6 addresses must be bracketed, which is how Fly
	/// 6PN addresses arrive.
	pub addresses: Option<Vec<String>>,

	/// Cluster description used when generating a cluster file from `addresses`.
	pub cluster_description: Option<String>,

	/// Cluster ID used when generating a cluster file from `addresses`.
	pub cluster_id: Option<String>,

	/// Where a generated cluster file is written.
	pub cluster_file_write_path: Option<PathBuf>,
}

impl FoundationDb {
	pub const DEFAULT_CLUSTER_FILE: &'static str = "/etc/foundationdb/fdb.cluster";

	/// Resolves the cluster file path, generating the file from `addresses` when one was not
	/// supplied directly.
	pub fn resolve_cluster_file(&self) -> anyhow::Result<PathBuf> {
		use anyhow::{Context, bail};

		if let Some(path) = &self.cluster_file {
			return Ok(path.clone());
		}

		let Some(addresses) = &self.addresses else {
			bail!(
				"foundationdb requires either `cluster_file` or `addresses` to be set (RIVET__FOUNDATIONDB__CLUSTER_FILE or RIVET__FOUNDATIONDB__ADDRESSES)"
			);
		};

		if addresses.is_empty() {
			bail!("foundationdb `addresses` is empty");
		}

		let description = self.cluster_description.as_deref().unwrap_or("rivet");
		let id = self.cluster_id.as_deref().unwrap_or("rivet");
		let contents = format!("{description}:{id}@{}\n", addresses.join(","));

		let path = self
			.cluster_file_write_path
			.clone()
			.unwrap_or_else(|| PathBuf::from(Self::DEFAULT_CLUSTER_FILE));

		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)
				.with_context(|| format!("failed to create {}", parent.display()))?;
		}

		std::fs::write(&path, contents)
			.with_context(|| format!("failed to write cluster file {}", path.display()))?;

		Ok(path)
	}
}

impl Default for FileSystem {
	fn default() -> Self {
		let default_path = dirs::data_local_dir()
			.map(|dir| dir.join("rivet-engine").join("db"))
			.unwrap_or_else(|| PathBuf::from("./data/db"));

		Self { path: default_path }
	}
}
