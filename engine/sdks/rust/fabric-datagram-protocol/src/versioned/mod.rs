use anyhow::{Result, bail};
use vbare::OwnedVersionedData;

use crate::generated::v1;

// Only one version exists, so there are no cross-version converters. The
// versioned wrapper still gives every peer a version header, so a v2 can be
// added later without changing the wire discipline.

// MARK: ToEnvoy

pub enum ToEnvoy {
	V1(v1::ToEnvoy),
}

impl OwnedVersionedData for ToEnvoy {
	type Latest = v1::ToEnvoy;

	fn wrap_latest(latest: Self::Latest) -> Self {
		Self::V1(latest)
	}

	fn unwrap_latest(self) -> Result<Self::Latest> {
		match self {
			Self::V1(x) => Ok(x),
		}
	}

	fn deserialize_version(payload: &[u8], version: u16) -> Result<Self> {
		match version {
			1 => Ok(Self::V1(serde_bare::from_slice(payload)?)),
			_ => bail!("invalid version: {version}"),
		}
	}

	fn serialize_version(self, _version: u16) -> Result<Vec<u8>> {
		match self {
			Self::V1(x) => serde_bare::to_vec(&x).map_err(Into::into),
		}
	}

	fn deserialize_converters() -> Vec<impl Fn(Self) -> Result<Self>> {
		Vec::<fn(Self) -> Result<Self>>::new()
	}

	fn serialize_converters() -> Vec<impl Fn(Self) -> Result<Self>> {
		Vec::<fn(Self) -> Result<Self>>::new()
	}
}

// MARK: ToRivet

pub enum ToRivet {
	V1(v1::ToRivet),
}

impl OwnedVersionedData for ToRivet {
	type Latest = v1::ToRivet;

	fn wrap_latest(latest: Self::Latest) -> Self {
		Self::V1(latest)
	}

	fn unwrap_latest(self) -> Result<Self::Latest> {
		match self {
			Self::V1(x) => Ok(x),
		}
	}

	fn deserialize_version(payload: &[u8], version: u16) -> Result<Self> {
		match version {
			1 => Ok(Self::V1(serde_bare::from_slice(payload)?)),
			_ => bail!("invalid version: {version}"),
		}
	}

	fn serialize_version(self, _version: u16) -> Result<Vec<u8>> {
		match self {
			Self::V1(x) => serde_bare::to_vec(&x).map_err(Into::into),
		}
	}

	fn deserialize_converters() -> Vec<impl Fn(Self) -> Result<Self>> {
		Vec::<fn(Self) -> Result<Self>>::new()
	}

	fn serialize_converters() -> Vec<impl Fn(Self) -> Result<Self>> {
		Vec::<fn(Self) -> Result<Self>>::new()
	}
}
