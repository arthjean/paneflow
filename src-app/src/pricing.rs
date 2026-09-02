use crate::agent_sessions::AssistantUsage;

pub const PRICING_TABLE_VERSION: &str = "2026-06-17";

#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

pub const PRICING_TABLE: &[(&str, ModelPricing)] = &[
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

pub fn lookup(model: &str) -> Option<ModelPricing> {
    let lc = model.to_ascii_lowercase();
    PRICING_TABLE
        .iter()
        .find(|(key, _)| lc.contains(key))
        .map(|(_, p)| *p)
}

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
        let opus = lookup("claude-opus-4-8-20260101").expect("opus priced");
        assert_eq!(opus.input, 15.0);
        let sonnet = lookup("claude-sonnet-4-6").expect("sonnet priced");
        assert_eq!(sonnet.input, 3.0);
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
