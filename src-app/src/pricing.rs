//! EP-004 US-017 (review redesign): a build-time-embedded,
//! versioned model pricing table for the Review attribution badge's estimated
//! cost. 100% local - no network lookup (a hard Paneflow constraint); a signed
//! remote manifest is out of scope unless model churn proves painful.
//!
//! Every figure here is an ESTIMATE. The badge always labels cost "~$X.XX
//! (est.)" and carries [`PRICING_TABLE_VERSION`]; an unknown model shows tokens
//! with NO cost (never a fabricated number - [`estimate_cost`] returns `None`).
//!
//! ## Updating the table
//!
//! 1. Bump [`PRICING_TABLE_VERSION`] to today's date (`YYYY-MM-DD`).
//! 2. Edit [`PRICING_TABLE`] rows; prices are US dollars per **million** tokens.
//! 3. Order rows specific → general: [`lookup`] returns the FIRST row whose
//!    `match_substr` is a case-insensitive substring of the model id, so a
//!    broad key (`"claude"`) must sit AFTER the narrow ones (`"opus"`).
//! 4. `cargo test -p paneflow-app pricing` covers the lookup ordering + the
//!    cost arithmetic.

use crate::agent_sessions::AssistantUsage;

/// Version stamp surfaced in the cost tooltip so a stale estimate is auditable.
pub const PRICING_TABLE_VERSION: &str = "2026-06-17";

/// Dollars per million tokens for one model family. `cache_read` is the
/// (cheaper) rate for cached-input tokens; `cache_write` is the (pricier) rate
/// for cache-creation tokens. Providers that bill cache differently are
/// normalized into these four tiers by the session scanners.
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// `(match_substr, pricing)` - ORDER MATTERS (specific → general). Public
/// figures as of [`PRICING_TABLE_VERSION`]; all estimates.
pub const PRICING_TABLE: &[(&str, ModelPricing)] = &[
    // ── Anthropic (Claude Code) ──────────────────────────────────────────
    (
        "opus",
        ModelPricing {
            input: 15.0,
            output: 75.0,
            cache_read: 1.50,
            cache_write: 18.75,
        },
    ),
    (
        "sonnet",
        ModelPricing {
            input: 3.0,
            output: 15.0,
            cache_read: 0.30,
            cache_write: 3.75,
        },
    ),
    (
        "haiku",
        ModelPricing {
            input: 0.80,
            output: 4.0,
            cache_read: 0.08,
            cache_write: 1.0,
        },
    ),
    // ── OpenAI (Codex) ───────────────────────────────────────────────────
    // GPT-5 family; `gpt` is the general fallback for other GPT models.
    (
        "gpt-5",
        ModelPricing {
            input: 1.25,
            output: 10.0,
            cache_read: 0.125,
            cache_write: 1.25,
        },
    ),
    (
        "gpt",
        ModelPricing {
            input: 1.25,
            output: 10.0,
            cache_read: 0.125,
            cache_write: 1.25,
        },
    ),
];

/// Look up pricing for a model id by case-insensitive substring against
/// [`PRICING_TABLE`] (first match wins - rows are ordered specific → general).
/// `None` for an unrecognized model.
pub fn lookup(model: &str) -> Option<ModelPricing> {
    let lc = model.to_ascii_lowercase();
    PRICING_TABLE
        .iter()
        .find(|(key, _)| lc.contains(key))
        .map(|(_, p)| *p)
}

/// Estimated cost in US dollars for `usage` under `model`'s pricing, or `None`
/// when the model is unpriced (the caller then shows tokens without a cost,
/// never a fabricated number). Per-tier: tokens / 1e6 × per-Mtok rate.
pub fn estimate_cost(model: &str, usage: &AssistantUsage) -> Option<f64> {
    let p = lookup(model)?;
    let per_m = |tokens: u64, rate: f64| (tokens as f64) / 1_000_000.0 * rate;
    Some(
        per_m(usage.input, p.input)
            + per_m(usage.output, p.output)
            + per_m(usage.cache_read, p.cache_read)
            + per_m(usage.cache_creation, p.cache_write),
    )
}

/// Format a dollar amount for the badge: `~$0.42` style. Two decimals, except
/// sub-cent amounts show three so a real-but-tiny cost never reads as `~$0.00`.
pub fn format_cost(dollars: f64) -> String {
    if dollars > 0.0 && dollars < 0.01 {
        format!("~${dollars:.3}")
    } else {
        format!("~${dollars:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_matches_specific_before_general() {
        // A Claude Opus id must hit the opus row, not a broader key.
        let opus = lookup("claude-opus-4-8-20260101").expect("opus priced");
        assert_eq!(opus.input, 15.0);
        let sonnet = lookup("claude-sonnet-4-6").expect("sonnet priced");
        assert_eq!(sonnet.input, 3.0);
        // gpt-5 hits the gpt-5 row (which precedes the general gpt fallback).
        let gpt5 = lookup("gpt-5").expect("gpt-5 priced");
        assert_eq!(gpt5.output, 10.0);
    }

    #[test]
    fn lookup_unknown_model_is_none() {
        assert!(lookup("llama-3-70b").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn estimate_cost_sums_tiers() {
        // 1M input + 1M output under sonnet ($3 + $15) = $18.
        let usage = AssistantUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 0,
            cache_creation: 0,
        };
        let cost = estimate_cost("claude-sonnet-4-6", &usage).expect("priced");
        assert!((cost - 18.0).abs() < 1e-9, "expected $18, got {cost}");
    }

    #[test]
    fn estimate_cost_unknown_model_none() {
        let usage = AssistantUsage {
            input: 1_000_000,
            ..Default::default()
        };
        assert!(estimate_cost("some-unknown-model", &usage).is_none());
    }

    #[test]
    fn format_cost_subcent_uses_three_decimals() {
        assert_eq!(format_cost(0.004), "~$0.004");
        assert_eq!(format_cost(0.42), "~$0.42");
        assert_eq!(format_cost(12.5), "~$12.50");
    }
}
