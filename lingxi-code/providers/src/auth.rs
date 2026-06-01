//! Per-provider auth-header seam. v1 supports API-key styles; signed auth
//! (Vertex / Bedrock / Azure AD) is added later behind the same seam.

/// How a provider authenticates a request.
#[derive(Debug, Clone)]
pub enum Auth {
    /// No auth header (e.g. a local Ollama endpoint).
    None,
    /// `Authorization: Bearer <key>` (`OpenAI` and OpenAI-compatible).
    Bearer(String),
    /// A literal header name/value pair (e.g. `x-goog-api-key`, `x-api-key`).
    Header {
        /// Header name.
        name: String,
        /// Header value.
        value: String,
    },
}

impl Auth {
    /// Append this auth's header(s) to `headers`. No-op for [`Auth::None`].
    pub fn apply(&self, headers: &mut Vec<(String, String)>) {
        match self {
            Self::None => {}
            Self::Bearer(key) => {
                headers.push(("authorization".to_string(), format!("Bearer {key}")));
            }
            Self::Header { name, value } => headers.push((name.clone(), value.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_appends_authorization_header() {
        let mut h = Vec::new();
        Auth::Bearer("sk-123".to_string()).apply(&mut h);
        assert_eq!(
            h,
            vec![("authorization".to_string(), "Bearer sk-123".to_string())]
        );
    }

    #[test]
    fn none_appends_nothing() {
        let mut h = Vec::new();
        Auth::None.apply(&mut h);
        assert!(h.is_empty());
    }
}
