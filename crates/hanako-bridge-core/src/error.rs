use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("{message}")]
    Tool {
        code: &'static str,
        message: String,
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl BridgeError {
    pub fn tool(code: &'static str, message: impl Into<String>) -> Self {
        Self::Tool {
            code,
            message: message.into(),
            expected: None,
            actual: None,
        }
    }

    pub fn mismatch(
        code: &'static str,
        message: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self::Tool {
            code,
            message: message.into(),
            expected: Some(expected.into()),
            actual: Some(actual.into()),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Tool { code, .. } => code,
            Self::Read { .. } => "read_failed",
            Self::Write { .. } => "write_failed",
            Self::Json { .. } => "invalid_json",
            Self::Other(_) => "bridge_error",
        }
    }

    pub fn expected(&self) -> Option<&str> {
        match self {
            Self::Tool { expected, .. } => expected.as_deref(),
            _ => None,
        }
    }

    pub fn actual(&self) -> Option<&str> {
        match self {
            Self::Tool { actual, .. } => actual.as_deref(),
            _ => None,
        }
    }
}

pub type BridgeResult<T> = Result<T, BridgeError>;
