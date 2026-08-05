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

// ---------------------------------------------------------------------------
// Produce lowering
// ---------------------------------------------------------------------------

/// A probe `produce` body lowered to the form a Lua VM evaluates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredProduce {
    /// The chunk name the body is loaded under (`@probe:{key}`).
    pub chunk_name: String,
    /// The wrapped source: `return (function()\n{body}\nend)()`.
    pub source: String,
}

/// Lower a probe's `produce` body for evaluation (§22.5.6).
///
/// Both phases run the same lowering: the register pre-pass on the register
/// VM (member-source resolution) and the executor on a worker VM (demand-
/// driven misses). The wrapper adds exactly ONE line above the body, and
/// that "exactly one" is load-bearing: a diagnostic raised on line N of the
/// wrapped chunk points at line N-1 of the author's `produce` body, and the
/// arithmetic must come out the same whichever phase ran it.
pub fn lower_produce(key: &str, produce: &str) -> LoweredProduce {
    LoweredProduce {
        chunk_name: format!("@probe:{key}"),
        source: format!("return (function()\n{produce}\nend)()"),
    }
}

#[cfg(test)]
#[path = "tests/produce_tests.rs"]
mod produce_tests;
