//! Byte-exact port of claude-code 2.1.206's cost-summary renderers
//! (`i6e`/`qs`/`cbg`/`FTu`/`Bu`). The whole block is rendered dimmed by the
//! consumer; these functions return plain strings.

/// Duration formatter — port of claude-code `qs(ms)` (no options). `ms` is an
/// integer, so the float sub-millisecond `.toFixed(1)` branch (`e < 1`) reduces
/// to the `e === 0` case already handled here.
#[must_use]
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 60_000 {
        return format!("{}s", ms / 1000);
    }
    let mut r = ms / 86_400_000;
    let mut n = (ms % 86_400_000) / 3_600_000;
    let mut o = (ms % 3_600_000) / 60_000;
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut i = ((ms % 60_000) as f64 / 1000.0).round() as u64;
    if i == 60 {
        i = 0;
        o += 1;
    }
    if o == 60 {
        o = 0;
        n += 1;
    }
    if n == 24 {
        n = 0;
        r += 1;
    }
    if r > 0 {
        return format!("{r}d {n}h {o}m");
    }
    if n > 0 {
        return format!("{n}h {o}m {i}s");
    }
    if o > 0 {
        return format!("{o}m {i}s");
    }
    if i > 0 {
        return format!("{i}s");
    }
    "0s".to_string()
}

/// Cost formatter — port of claude-code `FTu(usd, 4)`: `> $0.50` renders 2
/// decimals (rounded to cents via `dbg(e,100)=round(e*100)/100`); otherwise 4.
#[must_use]
pub fn format_cost(usd: f64) -> String {
    if usd > 0.5 {
        let cents = (usd * 100.0).round() / 100.0;
        format!("${cents:.2}")
    } else {
        format!("${usd:.4}")
    }
}

/// Token-count formatter — port of claude-code `Bu` (Intl compact notation,
/// lowercased, `maximumFractionDigits: 1`, trailing `.0` dropped). Identical
/// output to the port's existing `format_tokens`.
#[must_use]
pub fn format_token_count(n: u64) -> String {
    const UNITS: [(u64, char); 4] = [
        (1_000_000_000_000, 't'),
        (1_000_000_000, 'b'),
        (1_000_000, 'm'),
        (1_000, 'k'),
    ];
    for &(threshold, suffix) in &UNITS {
        if n >= threshold {
            #[allow(clippy::cast_precision_loss)]
            let rounded = ((n as f64 / threshold as f64) * 10.0).round() / 10.0;
            let s = format!("{rounded:.1}");
            let s = s.strip_suffix(".0").unwrap_or(&s);
            return format!("{s}{suffix}");
        }
    }
    n.to_string()
}

use crate::pricing::ModelRef;
use crate::summary::ModelCostSummary;

/// `nano-USD → USD` (matches `cost/src/pricing.rs`'s `nano / 1e9`).
#[allow(clippy::cast_precision_loss)]
fn nano_to_usd(nano: u64) -> f64 {
    nano as f64 / 1_000_000_000.0
}

/// Model display name for the "Usage by model" label. Uses the provider-scoped
/// model name as-is (the port's per-model key is already a display-usable
/// ref); a catalog lookup can refine this later without changing the format.
fn model_label(model_ref: &ModelRef) -> String {
    model_ref.model.clone()
}

/// Usage-by-model block — port of claude-code `cbg()`. Empty usage renders the
/// aligned zero line; otherwise a header plus one right-aligned line per model.
/// The per-model web-search clause is omitted (no per-model web-search tracking).
#[must_use]
pub fn usage_by_model_block(by_model: &[ModelCostSummary]) -> String {
    if by_model.is_empty() {
        return "Usage:                 0 input, 0 output, 0 cache read, 0 cache write".to_string();
    }
    let mut r = "Usage by model:".to_string();
    for m in by_model {
        let label = format!("{}:", model_label(&m.model_ref));
        // right-align label to width 21 (claude-code `padStart(21)`).
        let padded = format!("{label:>21}");
        let line = format!(
            "  {} input, {} output, {} cache read, {} cache write ({})",
            format_token_count(m.input_tokens),
            format_token_count(m.output_tokens),
            format_token_count(m.cache_read_input_tokens),
            format_token_count(m.cache_creation_input_tokens),
            format_cost(nano_to_usd(m.total_nano_usd)),
        );
        r.push('\n');
        r.push_str(&padded);
        r.push_str(&line);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::ProviderId;

    #[test]
    fn cbg_empty_and_per_model() {
        // Empty → the aligned zero line.
        assert_eq!(
            usage_by_model_block(&[]),
            "Usage:                 0 input, 0 output, 0 cache read, 0 cache write"
        );
        let rows = vec![ModelCostSummary {
            model_ref: ModelRef {
                provider: ProviderId::Anthropic,
                model: "claude-opus-4-8".into(),
            },
            total_nano_usd: 1_230_000_000, // $1.23
            input_tokens: 5_000,
            output_tokens: 2_000,
            cache_read_input_tokens: 1_500,
            cache_creation_input_tokens: 0,
        }];
        let out = usage_by_model_block(&rows);
        // label right-aligned to 21 ("claude-opus-4-8:" is 16 chars, so 5 leading spaces),
        // then the token/cost line (5k, 2k, 1.5k, 0; cost $1.23).
        assert_eq!(
            out,
            "Usage by model:\n     claude-opus-4-8:  5k input, 2k output, 1.5k cache read, 0 cache write ($1.23)"
        );
    }

    #[test]
    fn cbg_multiple_models_joined() {
        let rows = vec![
            ModelCostSummary {
                model_ref: ModelRef {
                    provider: ProviderId::Anthropic,
                    model: "claude-opus-4-8".into(),
                },
                total_nano_usd: 600_000_000, // $0.60
                input_tokens: 1_000,
                output_tokens: 500,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
            ModelCostSummary {
                model_ref: ModelRef {
                    provider: ProviderId::Anthropic,
                    model: "claude-sonnet-4-20250514".into(),
                },
                total_nano_usd: 400_000_000, // $0.40
                input_tokens: 2_000,
                output_tokens: 100,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        ];
        let out = usage_by_model_block(&rows);
        // header + two model lines, each starting on its own line.
        assert!(out.starts_with("Usage by model:\n"), "{out}");
        assert_eq!(out.matches('\n').count(), 2, "{out}"); // header\n line1\n line2 => 2 newlines
    }

    #[test]
    fn qs_matches_cc() {
        assert_eq!(format_duration_ms(0), "0s");
        assert_eq!(format_duration_ms(500), "0s");     // <1s floors to 0
        assert_eq!(format_duration_ms(1_500), "1s");   // floor(1.5)
        assert_eq!(format_duration_ms(59_000), "59s");
        assert_eq!(format_duration_ms(60_000), "1m 0s");
        assert_eq!(format_duration_ms(65_000), "1m 5s");
        assert_eq!(format_duration_ms(3_661_000), "1h 1m 1s");
        assert_eq!(format_duration_ms(90_061_000), "1d 1h 1m"); // days form drops seconds
        assert_eq!(format_duration_ms(59_500), "59s");  // floor(59.5)=59
        assert_eq!(format_duration_ms(119_500), "2m 0s"); // round(59.5s)=60 -> carry: 1m -> 2m 0s
    }

    #[test]
    fn ftu_matches_cc() {
        assert_eq!(format_cost(0.0), "$0.0000");
        assert_eq!(format_cost(0.05), "$0.0500");
        assert_eq!(format_cost(0.5), "$0.5000");        // not > 0.5
        assert_eq!(format_cost(0.5001), "$0.50");       // > 0.5 -> 2dp rounded
        assert_eq!(format_cost(1.2345), "$1.23");
        assert_eq!(format_cost(12.999), "$13.00");
    }

    #[test]
    fn bu_matches_cc_compact() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1_000), "1k");
        assert_eq!(format_token_count(1_500), "1.5k");
        assert_eq!(format_token_count(12_345), "12.3k");
        assert_eq!(format_token_count(50_000), "50k");
        assert_eq!(format_token_count(1_000_000), "1m");
        assert_eq!(format_token_count(1_500_000), "1.5m");
    }
}
