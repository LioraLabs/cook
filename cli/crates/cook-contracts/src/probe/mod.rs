//! Probe declarations and pure probe-value rules.

pub mod value;

/// Declared inputs for a probe unit.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeInputs {
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

/// A probe unit declared via `cook.probe(key, opts)`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeUnit {
    pub key: String,
    pub produce_source: String,
    pub produce_line: usize,
    pub inputs: ProbeInputs,
}
