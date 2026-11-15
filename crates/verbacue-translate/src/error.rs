//! The error type of verbacue-translate: the failures of online services that callers
//! react to.

use verbacue_core::error::ErrorKind;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("{service} needs a key: set {var} or write it to {file}")]
    MissingKey {
        service: &'static str,
        var: &'static str,
        file: String,
    },
    #[error(
        "Azure Translator needs the resource region: set AZURE_TRANSLATOR_REGION or write it \
         to {file}"
    )]
    MissingRegion { file: String },
    #[error("{service} refused the key ({status}): {body}")]
    KeyRefused {
        service: &'static str,
        status: u16,
        body: String,
    },
    #[error("the DeepL character quota is used up (456)")]
    QuotaExceeded,
    #[error("{service} answered {status}: {body}")]
    Status {
        service: &'static str,
        status: u16,
        body: String,
    },
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::MissingKey { .. } | Self::MissingRegion { .. } => ErrorKind::MissingKey,
            Self::KeyRefused { .. } => ErrorKind::KeyRefused,
            Self::QuotaExceeded => ErrorKind::QuotaExceeded,
            Self::Status { .. } => ErrorKind::Network,
        }
    }
}

impl From<Error> for verbacue_core::Error {
    fn from(e: Error) -> Self {
        Self::wrap(e.kind(), e)
    }
}
