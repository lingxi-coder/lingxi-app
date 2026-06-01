//! Pure-codec failure type. Distinct from `api_client::ApiError`: a
//! `CodecError` means the request could not even be encoded.

use thiserror::Error;

/// A failure while encoding a canonical request into a provider's wire shape.
#[derive(Debug, Clone, Error)]
pub enum CodecError {
    /// The request used a feature this provider/model cannot represent.
    #[error("unsupported request: {0}")]
    Unsupported(String),
    /// The request body could not be assembled.
    #[error("encode failed: {0}")]
    Encode(String),
}
