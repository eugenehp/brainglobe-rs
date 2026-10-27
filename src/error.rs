//! Error type shared by the whole crate.
//!
//! The variants mirror the Python exceptions brainglobe-atlasapi raises, so
//! callers can tell a `ValueError` from a `FileNotFoundError`, and the
//! messages are the Python ones (`str(exc)`).

use thiserror::Error;

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors raised by brainglobe-atlasapi.
#[derive(Debug, Error)]
pub enum Error {
    /// Python `ValueError`.
    #[error("{0}")]
    Value(String),
    /// Python `TypeError`.
    #[error("{0}")]
    Type(String),
    /// Python `KeyError`. Displayed as Python shows it: the key's repr.
    #[error("{0}")]
    Key(String),
    /// Python `IndexError`.
    #[error("{0}")]
    Index(String),
    /// Python `FileNotFoundError` raised with a message.
    #[error("{0}")]
    FileNotFound(String),
    /// Python `ConnectionError`.
    #[error("{0}")]
    Connection(String),
    /// Python `RuntimeError`.
    #[error("{0}")]
    Runtime(String),
    /// Python `AssertionError`.
    #[error("{0}")]
    Assertion(String),
    /// Operating system / IO errors (Python `OSError`).
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON errors (Python `json.JSONDecodeError`).
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Reading an OME-Zarr stack.
    #[cfg(feature = "zarr")]
    #[error(transparent)]
    OmeZarr(#[from] ome_zarr::Error),
    /// HTTP errors (requests' exceptions).
    #[error("{0}")]
    Http(String),
}

impl Error {
    pub fn value(msg: impl Into<String>) -> Self {
        Error::Value(msg.into())
    }
    /// A `KeyError` for `key`, displayed as Python's `str(KeyError(key))`:
    /// the key's repr (`'CH'` for a string, `5` for an int).
    pub fn key(key: &str) -> Self {
        Error::Key(crate::utils::py_repr_str(key))
    }
    pub fn key_int(key: i64) -> Self {
        Error::Key(key.to_string())
    }
    pub fn file_not_found(msg: impl Into<String>) -> Self {
        Error::FileNotFound(msg.into())
    }

    /// The Python exception class this error corresponds to.
    pub fn py_type(&self) -> &'static str {
        match self {
            Error::Value(_) => "ValueError",
            Error::Type(_) => "TypeError",
            Error::Key(_) => "KeyError",
            Error::Index(_) => "IndexError",
            Error::FileNotFound(_) => "FileNotFoundError",
            Error::Connection(_) => "ConnectionError",
            Error::Runtime(_) => "RuntimeError",
            Error::Assertion(_) => "AssertionError",
            Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound => "FileNotFoundError",
            Error::Io(_) => "OSError",
            Error::Json(_) => "JSONDecodeError",
            #[cfg(feature = "zarr")]
            Error::OmeZarr(e) => e.py_type(),
            Error::Http(_) => "HTTPError",
        }
    }
}
