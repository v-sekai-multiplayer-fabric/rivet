use foundationdb::FdbError;

/// Error codes we care about by name. See
/// https://apple.github.io/foundationdb/api-error-codes.html
pub const TRANSACTION_TOO_OLD: i32 = 1007;
pub const NOT_COMMITTED: i32 = 1020;
pub const COMMIT_UNKNOWN_RESULT: i32 = 1021;
pub const USED_DURING_COMMIT: i32 = 2017;
pub const TRANSACTION_TOO_LARGE: i32 = 2101;

/// Wraps `FdbError` so the retry loop can recover the original code after the error has been
/// converted into `anyhow::Error`.
#[derive(thiserror::Error, Debug)]
#[error("foundationdb error {code}: {source}")]
pub struct FdbDriverError {
	pub code: i32,
	#[source]
	pub source: FdbError,
}

impl FdbDriverError {
	pub fn is_retryable(&self) -> bool {
		self.source.is_retryable()
	}

	pub fn is_maybe_committed(&self) -> bool {
		self.source.is_maybe_committed()
	}

	pub fn is_transaction_too_large(&self) -> bool {
		self.code == TRANSACTION_TOO_LARGE
	}
}

impl From<FdbError> for FdbDriverError {
	fn from(source: FdbError) -> Self {
		FdbDriverError {
			code: source.code(),
			source,
		}
	}
}

pub fn map_err(err: FdbError) -> anyhow::Error {
	anyhow::Error::from(FdbDriverError::from(err))
}
