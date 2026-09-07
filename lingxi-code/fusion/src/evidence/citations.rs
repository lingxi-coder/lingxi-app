//! Validate the reserved citation syntax in a merged answer against the exact
//! host-verified receipt references included in the final synthesis payload.
//! This verifies reference provenance, never the truth of an interpretation.

use std::collections::BTreeSet;

const PREFIX: &[u8] = b"[evidence:";
const REF_PREFIX: &[u8] = b"evr_";
const HEX_LENGTH: usize = 32;
const REF_LENGTH: usize = REF_PREFIX.len() + HEX_LENGTH;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CitationValidation {
    /// No reserved citation marker appeared. This is NOT positive evidence
    /// verification and must remain distinguishable in the caller's result.
    NoCitations,
    /// Every cited reference was authorized; duplicates are removed. A caller
    /// must not interpret this as verification of the surrounding claims.
    ValidReferences { references: BTreeSet<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CitationError {
    /// A reserved marker is not exactly `[evidence:evr_<32 lower hex>]`.
    /// Offsets are UTF-8 byte offsets into the unchanged answer.
    Malformed { offset: usize },
    /// Syntax is valid, but the reference is absent from the final delivered
    /// allowlist (including unknown, cross-panel, and packed-out references).
    Unauthorized { offset: usize },
}

/// `allowed_refs` must contain only host-verified references actually delivered
/// in the FINAL synthesis request, after packing. A fetched receipt or a model
/// report's claimed reference is insufficient authority. Reference strings in
/// the set use the bare `evr_...` form, not the surrounding citation marker.
///
/// Quoted prose, code fences and backslashes do not disable this reserved
/// syntax: an unknown reference cannot bypass validation by being quoted.
/// Ordinary URLs, plain receipt strings and other Markdown links are ignored;
/// they cannot produce a positive citation-validation result.
///
/// The scan is strictly linear in answer bytes, apart from ordered-set lookup
/// and insertion. Each position has one constant-length prefix check; each
/// recognized marker examines exactly 32 hex bytes and then advances past it.
/// There is no suffix search, regex backtracking, or repeated whole-text scan.
pub(crate) fn validate_merged_citations(
    answer: &str,
    allowed_refs: &BTreeSet<String>,
) -> Result<CitationValidation, CitationError> {
    let bytes = answer.as_bytes();
    let mut offset = 0;
    let mut references = BTreeSet::new();
    while offset < bytes.len() {
        if !bytes[offset..].starts_with(PREFIX) {
            offset += 1;
            continue;
        }
        let body_start = offset + PREFIX.len();
        // The constant body and closing bracket must fit before adding their
        // length to an offset. This also rejects an unclosed/empty marker.
        if bytes.len() - body_start < REF_LENGTH + 1 {
            return Err(CitationError::Malformed { offset });
        }
        let body_end = body_start + REF_LENGTH;
        let body = &bytes[body_start..body_end];
        if !body.starts_with(REF_PREFIX)
            || !body[REF_PREFIX.len()..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            || bytes[body_end] != b']'
        {
            return Err(CitationError::Malformed { offset });
        }
        // The entire prefix/body is now known ASCII, hence both slice indices
        // are valid UTF-8 boundaries even when surrounding text contains CJK.
        let reference = &answer[body_start..body_end];
        if !allowed_refs.contains(reference) {
            return Err(CitationError::Unauthorized { offset });
        }
        references.insert(reference.to_owned());
        offset = body_end + 1;
    }
    if references.is_empty() {
        Ok(CitationValidation::NoCitations)
    } else {
        Ok(CitationValidation::ValidReferences { references })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "evr_0123456789abcdef0123456789abcdef";
    const B: &str = "evr_fedcba9876543210fedcba9876543210";

    fn allow(references: &[&str]) -> BTreeSet<String> {
        references
            .iter()
            .map(|reference| (*reference).into())
            .collect()
    }

    #[test]
    fn allows_only_exact_delivered_references_and_deduplicates() {
        let text = format!("[evidence:{B}] [evidence:{A}][evidence:{B}]");
        assert_eq!(
            validate_merged_citations(&text, &allow(&[A, B])),
            Ok(CitationValidation::ValidReferences {
                references: allow(&[A, B])
            })
        );
    }

    #[test]
    fn unknown_cross_panel_or_packed_out_reference_is_unauthorized() {
        let text = format!("[evidence:{B}]");
        // These provenance failures intentionally share one safe error: only
        // the host's final delivered allowlist can distinguish their origins.
        for allowed in [allow(&[]), allow(&[A]), allow(&[&format!("{B}0")])] {
            assert_eq!(
                validate_merged_citations(&text, &allowed),
                Err(CitationError::Unauthorized { offset: 0 })
            );
        }
    }

    #[test]
    fn malformed_empty_nested_unclosed_or_noncanonical_markers_fail() {
        let malformed = [
            "[evidence:".to_string(),
            "[evidence:]".into(),
            format!("[evidence:{A}"),
            format!("[evidence: {A}]"),
            format!("[evidence:{A} ]"),
            format!("[evidence:{A}0]"),
            format!("[evidence:{}]", &A[..A.len() - 1]),
            format!("[evidence:{}]", A.to_uppercase()),
            format!("[evidence:[evidence:{A}]]"),
            "[evidence:evr_0123456789abcdef0123456789abcdeg]".into(),
            "[evidence:evr_0123456789abcdef0123456789abcde中]".into(),
            "[evidence:evr_0123456789abcdef\n0123456789abcdef]".into(),
        ];
        for text in malformed {
            assert_eq!(
                validate_merged_citations(&text, &allow(&[A])),
                Err(CitationError::Malformed { offset: 0 }),
                "{text}"
            );
        }
    }

    #[test]
    fn no_citation_is_explicitly_not_verified() {
        for text in [
            "",
            "普通无引用的答案 🧪",
            A,
            "https://example.invalid/evr_0123456789abcdef0123456789abcdef",
            "[ordinary markdown](https://example.invalid)",
        ] {
            assert_eq!(
                validate_merged_citations(text, &allow(&[A])),
                Ok(CitationValidation::NoCitations)
            );
        }
    }

    #[test]
    fn quote_code_or_escape_context_does_not_bypass_validation() {
        for (prefix, suffix) in [("\"", "\""), ("```\n", "\n```"), ("\\", ""), ("'", "'")] {
            let text = format!("{prefix}[evidence:{B}]{suffix}");
            assert_eq!(
                validate_merged_citations(&text, &allow(&[A])),
                Err(CitationError::Unauthorized {
                    offset: prefix.len()
                })
            );
            assert!(matches!(
                validate_merged_citations(&text, &allow(&[B])),
                Ok(CitationValidation::ValidReferences { .. })
            ));
        }
    }

    #[test]
    fn cjk_byte_offsets_and_later_invalid_marker_are_preserved() {
        let prefix = format!("结论🧪[evidence:{A}] 接着：");
        let text = format!("{prefix}[evidence:{B}]");
        assert_eq!(
            validate_merged_citations(&text, &allow(&[A])),
            Err(CitationError::Unauthorized {
                offset: prefix.len()
            })
        );
        let text = format!("{prefix}[evidence:]");
        assert_eq!(
            validate_merged_citations(&text, &allow(&[A])),
            Err(CitationError::Malformed {
                offset: prefix.len()
            })
        );
    }

    #[test]
    fn repeated_near_prefixes_and_duplicate_markers_remain_bounded() {
        // No timing assertion: stress repeated near-matches without a regex
        // engine or suffix searches, then verify exact deduplication.
        let text = format!(
            "{}{}",
            "[evidenc 🧪".repeat(10_000),
            format!("[evidence:{A}]").repeat(10_000)
        );
        assert_eq!(
            validate_merged_citations(&text, &allow(&[A])),
            Ok(CitationValidation::ValidReferences {
                references: allow(&[A])
            })
        );
    }
}
