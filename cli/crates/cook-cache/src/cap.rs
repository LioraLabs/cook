//! `max_artifact_bytes` enforcement, once (COOK-417).
//!
//! Both backends refuse an oversized artifact mid-stream rather than
//! pre-flight, because a caller may not know the size up front (CS-0057). The
//! two loops around that check genuinely differ: the local one also hashes and
//! writes a temp file, the cloud one feeds `ureq::Request::send`. What did not
//! differ, and was written three times, is the decision itself and the message
//! that reports it.
//!
//! `cloud_backend.rs` conceded the copy in a comment ("same shape as
//! `LocalBackend::put`'s inner-loop check") without an agreement test or a
//! named twin, which is the deliberate-copy protocol unmet on every condition.
//! There was no rejected home to justify it: this is a pure accumulator over
//! two integers.

/// Running byte total against a cap.
///
/// `add` returns the diagnostic when the total crosses the limit, so the
/// message exists in exactly one place. It is a `String` rather than a typed
/// error because the two call sites wrap it differently: one in
/// `BackendError::Other`, one in an `io::Error` that has to survive a round
/// trip through `ureq`'s transport error.
#[derive(Debug, Clone, Copy)]
pub struct CapCounter {
    total: u64,
    limit: u64,
}

impl CapCounter {
    pub fn new(limit: u64) -> Self {
        Self { total: 0, limit }
    }

    /// Add `n` bytes. `Err(message)` once the running total exceeds the cap.
    ///
    /// Saturating, so a pathological reader cannot wrap the total back under
    /// the limit.
    pub fn add(&mut self, n: u64) -> Result<(), String> {
        self.total = self.total.saturating_add(n);
        if self.total > self.limit {
            return Err(self.message());
        }
        Ok(())
    }

    /// The diagnostic for the current total. Public so the cloud path can
    /// re-raise it after `ureq` has flattened the `io::Error` into a transport
    /// error, without spelling the text a second time.
    pub fn message(&self) -> String {
        format!(
            "artifact exceeds max_artifact_bytes ({}); cap {}",
            self.total, self.limit
        )
    }

    /// True once the cap has been crossed.
    pub fn exceeded(&self) -> bool {
        self.total > self.limit
    }

    pub fn total(&self) -> u64 {
        self.total
    }
}

#[cfg(test)]
#[path = "tests/cap_tests.rs"]
mod tests;
