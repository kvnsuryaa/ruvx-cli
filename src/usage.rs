//! Provider usage stays intact in storage; the UI displays only reported aggregate counts.
use serde_json::Value;
#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub total: Option<u64>,
    pub cached: Option<u64>,
    pub reasoning: Option<u64>,
}
impl Usage {
    /// Prefer the provider total, or sum complete input/output aggregates only.
    pub fn total_tokens(&self) -> Option<u64> {
        self.total.or_else(|| self.input?.checked_add(self.output?))
    }

    pub fn from_value(value: &Value) -> Self {
        Self {
            input: value["input_tokens"]
                .as_u64()
                .or_else(|| value["prompt_tokens"].as_u64()),
            output: value["output_tokens"]
                .as_u64()
                .or_else(|| value["completion_tokens"].as_u64()),
            total: value["total_tokens"].as_u64(),
            cached: value["input_tokens_details"]["cached_tokens"]
                .as_u64()
                .or_else(|| value["prompt_tokens_details"]["cached_tokens"].as_u64()),
            reasoning: value["output_tokens_details"]["reasoning_tokens"]
                .as_u64()
                .or_else(|| value["completion_tokens_details"]["reasoning_tokens"].as_u64()),
        }
    }
    pub fn label(&self) -> String {
        let mut parts = vec!["Tokens".to_owned()];
        for (label, value) in [
            ("input", self.input),
            ("output", self.output),
            ("total", self.total),
        ] {
            if let Some(value) = value {
                parts.push(format!("{label} {value}"));
            }
        }
        parts.join(" · ")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn aggregates_do_not_sum_attribution_or_fabricate_zero() {
        let usage = Usage::from_value(
            &json!({"input_tokens":413,"output_tokens":14,"total_tokens":427,"attribution":{"input_tokens":9000},"output_tokens_details":{"reasoning_tokens":0}}),
        );
        assert_eq!(usage.label(), "Tokens · input 413 · output 14 · total 427");
        assert_eq!(usage.reasoning, Some(0));
        assert_eq!(usage.cached, None);
        assert_eq!(
            Usage::from_value(&json!({"prompt_tokens":12,"completion_tokens":0})).label(),
            "Tokens · input 12 · output 0"
        );
        assert_eq!(Usage::from_value(&json!({})).label(), "Tokens");
    }

    #[test]
    fn totals_require_complete_counts_and_never_double_count_nested_details() {
        for (value, expected) in [
            (
                json!({"total_tokens":42,"input_tokens":10,"output_tokens":5}),
                Some(42),
            ),
            (
                json!({"input_tokens":10,"output_tokens":5,"input_tokens_details":{"cached_tokens":7},"output_tokens_details":{"reasoning_tokens":3}}),
                Some(15),
            ),
            (json!({"prompt_tokens":12,"completion_tokens":0}), Some(12)),
            (json!({"total_tokens":0}), Some(0)),
            (json!({"input_tokens":12}), None),
            (json!({}), None),
            (json!({"input_tokens":u64::MAX,"output_tokens":1}), None),
        ] {
            assert_eq!(Usage::from_value(&value).total_tokens(), expected);
        }
    }
}
