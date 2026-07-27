//! Surface step-kind identity.

/// Which Cookfile step kind a unit was captured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StepKind {
    /// `cook` step body — cacheable, hermetic, sandboxed.
    Cook,
    /// `test` step body — non-cacheable but hermetic-by-intent.
    Test,
    /// `chore` body — non-cacheable, hermetic-by-intent.
    Chore,
}
