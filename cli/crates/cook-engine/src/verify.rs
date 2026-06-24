//! COOK-167 — `cook cache verify`: determinant-fidelity verifier.
//!
//! Re-runs each cacheable unit in a throwaway sandbox and byte-compares its
//! produced outputs against what the local cache recorded for the SAME key K
//! (populated by a normal `run()` against this very workspace, so K matches by
//! construction). A non-`record` unit whose re-run produces different bytes is a
//! divergence (§17.1.1 byte-equivalence broken — an under-keyed determinant or a
//! non-reproducible producer). A `record` unit (§17.1.4) is byte-exempt and gets
//! a weaker exists/non-empty check. This is a fidelity/diagnostic tool, NOT a
//! trust gate (COOK-167).

/// Per-unit verdict from the re-run-and-compare pass.
#[derive(Debug, Clone, PartialEq)]
pub enum UnitVerdict {
    /// Re-run reproduced byte-identical outputs under the matching key.
    Pass,
    /// `record` unit: byte-equivalence waived (§17.1.4); re-run produced the
    /// declared outputs and they were non-empty.
    RecordExempt,
    /// Re-run produced different bytes under a matching key — the cache would
    /// serve a non-byte-equivalent artifact. Under-keyed determinant or
    /// non-reproducible producer.
    Divergence { detail: String },
    /// The unit could not be verified (re-run failed, missing recorded entry,
    /// sandbox error). Distinct from a divergence — the producer broke.
    Error { detail: String },
}

impl UnitVerdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, UnitVerdict::Pass | UnitVerdict::RecordExempt)
    }
    pub fn label(&self) -> &'static str {
        match self {
            UnitVerdict::Pass => "pass",
            UnitVerdict::RecordExempt => "pass (record: byte-check waived)",
            UnitVerdict::Divergence { .. } => "DIVERGENCE",
            UnitVerdict::Error { .. } => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnitReport {
    pub recipe: String,
    pub unit: String,
    pub key: String,
    pub verdict: UnitVerdict,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub units: Vec<UnitReport>,
}

impl VerifyReport {
    /// 0 iff every unit verdict is_ok().
    pub fn exit_code(&self) -> i32 {
        if self.units.iter().all(|u| u.verdict.is_ok()) { 0 } else { 1 }
    }
    pub fn divergences(&self) -> usize {
        self.units.iter().filter(|u| matches!(u.verdict, UnitVerdict::Divergence { .. })).count()
    }
    pub fn errors(&self) -> usize {
        self.units.iter().filter(|u| matches!(u.verdict, UnitVerdict::Error { .. })).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_pass_is_ok_and_record_exempt_is_ok() {
        assert!(UnitVerdict::Pass.is_ok());
        assert!(UnitVerdict::RecordExempt.is_ok());
        assert!(!UnitVerdict::Divergence { detail: "x".into() }.is_ok());
        assert!(!UnitVerdict::Error { detail: "y".into() }.is_ok());
    }

    #[test]
    fn report_exit_code_zero_iff_all_ok() {
        let mut r = VerifyReport::default();
        r.units.push(UnitReport { recipe: "build".into(), unit: "a.o".into(), key: "k".into(), verdict: UnitVerdict::Pass });
        assert_eq!(r.exit_code(), 0);
        r.units.push(UnitReport { recipe: "build".into(), unit: "b.o".into(), key: "k2".into(), verdict: UnitVerdict::Divergence { detail: "bytes differ".into() } });
        assert_ne!(r.exit_code(), 0);
    }
}
