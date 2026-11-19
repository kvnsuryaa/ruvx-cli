//! Reasoning capability presets; unknown models require provider metadata or explicit configuration.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
impl ReasoningEffort {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::None => "No reasoning, when supported",
            Self::Minimal => "Minimal reasoning",
            Self::Low => "Faster, lighter reasoning",
            Self::Medium => "Balance speed and depth",
            Self::High => "Deeper reasoning",
            Self::Xhigh => "Extra reasoning depth",
            Self::Max => "Maximum reasoning depth",
        }
    }
}
impl fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for ReasoningEffort {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|level|level.as_str()==value).ok_or_else(||anyhow::anyhow!("Unknown effort '{value}'; choose none, minimal, low, medium, high, xhigh, or max"))
    }
}

/// Exact model IDs and valid dated snapshots only; never infer support from an arbitrary prefix.
/// Sources and verification date: docs/model-efforts.md (2026-11-19).
pub fn openai_levels(model: &str) -> Option<&'static [ReasoningEffort]> {
    use ReasoningEffort::*;
    const DEEP: &[ReasoningEffort] = &[Low, Medium, High, Xhigh, Max];
    const BALANCED: &[ReasoningEffort] = &[None, Low, Medium, High, Xhigh];
    const PRESETS: &[(&str, &[ReasoningEffort])] = &[
        ("gpt-6.2-astra", DEEP),
        ("gpt-6.2-sol", DEEP),
        ("gpt-6-astra", DEEP),
        ("gpt-6.1-sol", DEEP),
        ("gpt-6.1-terra", DEEP),
        ("gpt-6-sol", DEEP),
        ("gpt-6-luna", DEEP),
        ("gpt-5.6-sol", DEEP),
        ("gpt-5.6-terra", DEEP),
        ("gpt-5.6-luna", DEEP),
        ("gpt-5.5", BALANCED),
        ("gpt-5.2", &[None, Low, Medium, High, Xhigh]),
        ("gpt-5.1", &[None, Low, Medium, High]),
        ("gpt-5", &[Minimal, Low, Medium, High]),
    ];
    PRESETS.iter().find_map(|(id, levels)| {
        let snapshot = model
            .strip_prefix(id)
            .and_then(|suffix| suffix.strip_prefix('-'))
            .is_some_and(|date| {
                date.len() == 10 && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
            });
        (model == *id || snapshot).then_some(*levels)
    })
}

pub fn parse_levels(text: &str) -> Result<Vec<ReasoningEffort>> {
    if text.trim() == "unsupported" {
        return Ok(vec![]);
    }
    let mut levels = vec![];
    for value in text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|v| !v.is_empty())
    {
        let level = value.parse()?;
        if levels.contains(&level) {
            bail!("Duplicate effort '{value}'");
        }
        levels.push(level);
    }
    if levels.is_empty() {
        bail!("Enter supported levels separated by commas/spaces, or unsupported");
    }
    Ok(levels)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_are_model_specific_and_do_not_guess_aliases() {
        assert!(
            !openai_levels("gpt-5")
                .unwrap()
                .contains(&ReasoningEffort::None)
        );
        assert!(
            openai_levels("gpt-5.2-2025-12-11")
                .unwrap()
                .contains(&ReasoningEffort::Xhigh)
        );
        assert!(
            !openai_levels("gpt-5.1")
                .unwrap()
                .contains(&ReasoningEffort::Xhigh)
        );
        assert!(
            !openai_levels("gpt-6.2-astra")
                .unwrap()
                .contains(&ReasoningEffort::None)
        );
        assert!(openai_levels("gpt-5.2-custom").is_none());
        assert!(openai_levels("gpt-5.2-2025-99-99").is_none());
        assert!(openai_levels("gateway/gpt-5.2").is_none());
    }
    #[test]
    fn capability_input_distinguishes_none_from_unsupported() {
        assert_eq!(
            parse_levels("none, low high").unwrap(),
            vec![
                ReasoningEffort::None,
                ReasoningEffort::Low,
                ReasoningEffort::High
            ]
        );
        assert!(parse_levels("unsupported").unwrap().is_empty());
        assert!(parse_levels("").is_err());
        assert!(parse_levels("high,high").is_err());
        assert!(parse_levels("ultra").is_err());
    }
}
