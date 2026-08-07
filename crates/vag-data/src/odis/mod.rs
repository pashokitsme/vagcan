//! A read-only reader for a VW ODIS-Service runtime project.
//!
//! An extracted ODIS project is a directory of `<PoolID>.db` / `<PoolID>.key`
//! pairs plus two plaintext string pools. Nothing in it is encrypted
//! (`research/labels/odis-crib.md` §2); the three layers are a B+Tree index
//! ([`keyfile`], Peter Graf's PBL), zlib members ([`pool`]), and a positional
//! object stream ([`object`]) whose field order per type was reverse-engineered
//! by `ODIS-project-explorer` against a decompiled MCD kernel.
//!
//! ## Read-only, in two senses
//! Nothing here writes to a project — there is no insert, delete or split path
//! in the B+Tree at all. And nothing here parses an object type whose only
//! purpose is a write service: flashing, access keys, adaptation and coding
//! cases are refused by name in [`loaders::refused_type_name`], permanently.
//! See `SAFETY.md` and the design's §2.

pub mod hash;
pub mod keyfile;
pub mod object;
pub mod pool;
pub mod strings;

/// Everything that can go wrong reading a project.
///
/// Hand-rolled in the style of `vag_db::Error` — `vag-data` has no `anyhow` and
/// gains none here. Every variant carries enough to name the file or the field
/// that failed, because "the project is broken" is not a usable message when a
/// project is 472 files.
#[derive(Debug)]
pub enum Error {
	/// A file could not be read.
	Io(std::io::Error),
	/// A file was read but does not hold what its format promises: a truncated
	/// buffer, a length that overruns, a missing terminator, an enum value that
	/// is not one of the defined ones.
	Format(String),
	/// Something the project should contain was not there: a pool, a named
	/// object, a reference's target.
	Missing(String),
}

impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Error::Io(e) => write!(f, "io error: {e}"),
			Error::Format(m) => write!(f, "malformed ODIS project: {m}"),
			Error::Missing(m) => write!(f, "not in this ODIS project: {m}"),
		}
	}
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
	fn from(e: std::io::Error) -> Self {
		Error::Io(e)
	}
}
