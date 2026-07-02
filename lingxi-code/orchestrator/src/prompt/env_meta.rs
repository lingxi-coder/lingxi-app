//! Pure `<env>`-block metadata lookups for the production system-prompt
//! builder: marketing name + knowledge cutoff per model id, and the
//! `uname -sr` OS-version string.
//!
//! Ported 1:1 from claude-code:
//! - `getMarketingNameForModel` (`utils/model/model.ts:570-614`)
//! - `getKnowledgeCutoff` (`constants/prompts.ts:712-730`)
//! - `getUnameSR` (`constants/prompts.ts:745-756`)
//!
//! The [`env_block`](crate::prompt::env_block) formatter is unchanged; it
//! consumes whatever values the builder places in
//! [`SystemPromptContext`](crate::prompt::SystemPromptContext). Previously
//! the builder stubbed `model_marketing_name`/`knowledge_cutoff` to `None`
//! and `os_version` to `"<os> <arch>"`; these helpers feed the real values.
#![forbid(unsafe_code)]

/// Marketing name for a model id, e.g. `claude-opus-4-6` -> `Opus 4.6`.
///
/// Mirrors `getMarketingNameForModel`. The TS resolves the id to a canonical
/// short name first (`getCanonicalName`, which unwraps Bedrock/Vertex ARNs);
/// without that provider-resolution layer we substring-match the lowercased
/// id directly, which is equivalent for first-party ids. The `[1m]` 1M-context
/// suffix is detected on the lowercased id (TS `modelId.toLowerCase()`).
///
/// Order is significant — every later needle is a substring of an earlier one,
/// so the most specific suffix must be checked first. Returns `None` for an
/// unknown model (TS falls back to `You are powered by the model {id}.`).
#[must_use]
pub fn marketing_name_for_model(model_id: &str) -> Option<&'static str> {
    let canonical = model_id.to_ascii_lowercase();
    let has_1m = canonical.contains("[1m]");

    if canonical.contains("claude-fable-5") {
        return Some("Fable 5");
    }
    if canonical.contains("claude-mythos-5") {
        return Some("Mythos 5");
    }
    if canonical.contains("claude-opus-4-8") {
        return Some(if has_1m {
            "Opus 4.8 (1M context)"
        } else {
            "Opus 4.8"
        });
    }
    if canonical.contains("claude-opus-4-7") {
        return Some(if has_1m {
            "Opus 4.7 (1M context)"
        } else {
            "Opus 4.7"
        });
    }
    if canonical.contains("claude-opus-4-6") {
        return Some(if has_1m {
            "Opus 4.6 (1M context)"
        } else {
            "Opus 4.6"
        });
    }
    if canonical.contains("claude-opus-4-5") {
        return Some("Opus 4.5");
    }
    if canonical.contains("claude-opus-4-1") {
        return Some("Opus 4.1");
    }
    if canonical.contains("claude-opus-4") {
        return Some("Opus 4");
    }
    // sonnet-5 before the sonnet-4-x arms (2.1.198 registry display_name
    // "Sonnet 5"; mutually exclusive substrings).
    if canonical.contains("claude-sonnet-5") {
        return Some(if has_1m {
            "Sonnet 5 (1M context)"
        } else {
            "Sonnet 5"
        });
    }
    if canonical.contains("claude-sonnet-4-6") {
        return Some(if has_1m {
            "Sonnet 4.6 (1M context)"
        } else {
            "Sonnet 4.6"
        });
    }
    if canonical.contains("claude-sonnet-4-5") {
        return Some(if has_1m {
            "Sonnet 4.5 (1M context)"
        } else {
            "Sonnet 4.5"
        });
    }
    if canonical.contains("claude-sonnet-4") {
        return Some(if has_1m {
            "Sonnet 4 (1M context)"
        } else {
            "Sonnet 4"
        });
    }
    if canonical.contains("claude-3-7-sonnet") {
        return Some("Claude 3.7 Sonnet");
    }
    if canonical.contains("claude-3-5-sonnet") {
        return Some("Claude 3.5 Sonnet");
    }
    if canonical.contains("claude-haiku-4-5") {
        return Some("Haiku 4.5");
    }
    if canonical.contains("claude-3-5-haiku") {
        return Some("Claude 3.5 Haiku");
    }
    None
}

/// Knowledge-cutoff string for a model id, e.g. `claude-opus-4-6` ->
/// `May 2025`. Mirrors `getKnowledgeCutoff`. `None` for unknown models (TS
/// omits the `Assistant knowledge cutoff is ...` sentence entirely).
#[must_use]
pub fn knowledge_cutoff_for_model(model_id: &str) -> Option<&'static str> {
    let canonical = model_id.to_ascii_lowercase();
    if canonical.contains("claude-fable-5") || canonical.contains("claude-mythos-5") {
        Some("January 2026")
    } else if canonical.contains("claude-sonnet-5") {
        // 2.1.198 registry: claude-sonnet-5 knowledge_cutoff = "January 2026".
        Some("January 2026")
    } else if canonical.contains("claude-opus-4-8") || canonical.contains("claude-opus-4-7") {
        // TS lists `claude-opus-4-8` and `claude-opus-4-7` as separate arms
        // that both return "January 2026"; merged here to satisfy clippy
        // (if_same_then_else) — output is identical.
        Some("January 2026")
    } else if canonical.contains("claude-sonnet-4-6") {
        Some("August 2025")
    } else if canonical.contains("claude-opus-4-6") || canonical.contains("claude-opus-4-5") {
        // TS lists `claude-opus-4-6` and `claude-opus-4-5` as separate arms
        // that both return "May 2025"; merged here to satisfy clippy
        // (if_same_then_else) — output is identical.
        Some("May 2025")
    } else if canonical.contains("claude-haiku-4") {
        Some("February 2025")
    } else if canonical.contains("claude-opus-4") || canonical.contains("claude-sonnet-4") {
        Some("January 2025")
    } else {
        None
    }
}

/// `uname -sr`-style OS version string, e.g. `Darwin 25.3.0` / `Linux 6.6.4`.
///
/// Mirrors `getUnameSR`: on POSIX, `os.type()` + `os.release()` is byte-equal
/// to `uname -s -r`, so we shell out to that (one space-joined line) and trim.
/// On Windows (no `uname`) or any spawn/exit failure, fall back to the prior
/// `"<os> <arch>"` stub so the OS Version line is always populated.
#[must_use]
pub fn os_version_string() -> String {
    if let Ok(out) = std::process::Command::new("uname")
        .arg("-s")
        .arg("-r")
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Local-date string in `YYYY-MM-DD` form, e.g. `2026-06-20`.
///
/// 1:1 with claude-code `Rtt` (binary offset ~197026183), which builds
/// `${getFullYear()}-${getMonth()+1 padStart2}-${getDate() padStart2}` from a
/// `new Date()` — i.e. the LOCAL date (not UTC). This is the `${date}` body of
/// the `currentDate` additional-context entry (`WNi`: `Today's date is
/// ${date}.`).
#[must_use]
pub fn current_date_string() -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_date_is_iso_local() {
        let d = current_date_string();
        // YYYY-MM-DD: 10 chars, dashes at 4 and 7.
        assert_eq!(d.len(), 10, "got {d}");
        assert_eq!(&d[4..5], "-");
        assert_eq!(&d[7..8], "-");
        assert!(d[..4].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn marketing_names_match_ts_map() {
        assert_eq!(marketing_name_for_model("claude-fable-5"), Some("Fable 5"));
        assert_eq!(marketing_name_for_model("claude-mythos-5"), Some("Mythos 5"));
        assert_eq!(marketing_name_for_model("claude-opus-4-8"), Some("Opus 4.8"));
        assert_eq!(
            marketing_name_for_model("claude-opus-4-8-20260101[1m]"),
            Some("Opus 4.8 (1M context)")
        );
        assert_eq!(marketing_name_for_model("claude-opus-4-7"), Some("Opus 4.7"));
        assert_eq!(
            marketing_name_for_model("claude-opus-4-7-20251101[1m]"),
            Some("Opus 4.7 (1M context)")
        );
        assert_eq!(marketing_name_for_model("claude-opus-4-6"), Some("Opus 4.6"));
        assert_eq!(
            marketing_name_for_model("claude-opus-4-6-20251101[1m]"),
            Some("Opus 4.6 (1M context)")
        );
        assert_eq!(marketing_name_for_model("claude-opus-4-5"), Some("Opus 4.5"));
        assert_eq!(marketing_name_for_model("claude-opus-4-1"), Some("Opus 4.1"));
        assert_eq!(marketing_name_for_model("claude-opus-4-0"), Some("Opus 4"));
        assert_eq!(
            marketing_name_for_model("claude-sonnet-4-5[1m]"),
            Some("Sonnet 4.5 (1M context)")
        );
        assert_eq!(marketing_name_for_model("claude-sonnet-4-5"), Some("Sonnet 4.5"));
        assert_eq!(marketing_name_for_model("claude-sonnet-5"), Some("Sonnet 5"));
        assert_eq!(
            marketing_name_for_model("claude-sonnet-5[1m]"),
            Some("Sonnet 5 (1M context)")
        );
        assert_eq!(marketing_name_for_model("claude-haiku-4-5"), Some("Haiku 4.5"));
        assert_eq!(
            marketing_name_for_model("claude-3-7-sonnet"),
            Some("Claude 3.7 Sonnet")
        );
        // Unknown / unmapped -> None (TS bare-id fallback).
        assert_eq!(marketing_name_for_model("gpt-4o"), None);
    }

    #[test]
    fn knowledge_cutoffs_match_ts_map() {
        assert_eq!(knowledge_cutoff_for_model("claude-fable-5"), Some("January 2026"));
        assert_eq!(knowledge_cutoff_for_model("claude-mythos-5"), Some("January 2026"));
        assert_eq!(knowledge_cutoff_for_model("claude-opus-4-8"), Some("January 2026"));
        assert_eq!(knowledge_cutoff_for_model("claude-opus-4-7"), Some("January 2026"));
        assert_eq!(knowledge_cutoff_for_model("claude-sonnet-5"), Some("January 2026"));
        assert_eq!(knowledge_cutoff_for_model("claude-sonnet-4-6"), Some("August 2025"));
        assert_eq!(knowledge_cutoff_for_model("claude-opus-4-6"), Some("May 2025"));
        assert_eq!(knowledge_cutoff_for_model("claude-opus-4-5"), Some("May 2025"));
        assert_eq!(knowledge_cutoff_for_model("claude-haiku-4-5"), Some("February 2025"));
        assert_eq!(knowledge_cutoff_for_model("claude-opus-4-1"), Some("January 2025"));
        assert_eq!(knowledge_cutoff_for_model("claude-sonnet-4-0"), Some("January 2025"));
        assert_eq!(knowledge_cutoff_for_model("claude-sonnet-4-5"), Some("January 2025"));
        assert_eq!(knowledge_cutoff_for_model("gpt-4o"), None);
    }

    #[test]
    fn os_version_is_populated() {
        // Shells out to `uname` on this POSIX host; never empty (fallback
        // guarantees a value even on spawn failure).
        assert!(!os_version_string().is_empty());
    }
}
