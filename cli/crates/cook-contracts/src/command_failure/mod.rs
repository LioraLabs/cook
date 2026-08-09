use crate::CapturedStream;
use serde::{Deserialize, Serialize};

const MARKER: &str = "COOK_CMD_FAILED:";

/// A command failure shared between execution runtimes and presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandFailure {
    line: usize,
    exit_code: i32,
    command: String,
    stdout: CapturedStream,
    stderr: CapturedStream,
}

#[derive(Serialize, Deserialize)]
struct WireFailure {
    line: usize,
    exit_code: i32,
    command: String,
    stdout: String,
    stderr: String,
}

impl CommandFailure {
    pub fn new(
        line: usize,
        exit_code: i32,
        command: impl Into<String>,
        stdout: CapturedStream,
        stderr: CapturedStream,
    ) -> Self {
        Self {
            line,
            exit_code,
            command: command.into(),
            stdout,
            stderr,
        }
    }

    pub fn line(&self) -> usize {
        self.line
    }

    /// The line, when there is one.
    ///
    /// Zero is not a line: it is what a producer stores when it could not
    /// determine one. §{lua.cook-sh} (CS-0211) requires a failure it cannot
    /// locate to be reported WITHOUT a location rather than with a
    /// substitute, and "0" printed into a diagnostic is exactly the
    /// substitute — a reader cannot tell it from a real line 0 that does not
    /// exist. Every renderer asks this rather than testing `line() == 0`
    /// itself, because that test is one decision and it had two answers:
    /// cook-cli omitted the location and cook-engine's progress line printed
    /// `command at line 0`.
    pub fn located(&self) -> Option<usize> {
        (self.line != 0).then_some(self.line)
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    /// The command as a reader is shown it: the author's body, without the
    /// prelude [`crate::shell_block::compose`] put in front of it.
    ///
    /// `set -e` is the implementation's line, not the author's, and §{lua.cook-sh}
    /// (CS-0215) forbids showing it. A renderer holding a `CommandFailure`
    /// asks this rather than calling `strip_set_e` on `command()` itself, for
    /// the reason `located` gives one paragraph up: that was one decision with
    /// four implementations, and they disagreed. `cook-cli`'s final diagnostic
    /// stripped; `cook-engine`'s progress line printed the prelude as the
    /// first line of the failing command; `cook-cli`'s test report and
    /// `cook-plan`'s duplicate-output diagnostic each kept a private stripper,
    /// one missing the empty-block case and the other dropping the prefix
    /// without its newline, so `set -euo pipefail` rendered as `uo pipefail`.
    /// The two sites that hold a shell payload rather than a failure call
    /// [`crate::shell_block::strip_set_e`] directly; this is the same law one
    /// level up, for callers that have the failure.
    pub fn displayed_command(&self) -> &str {
        crate::shell_block::strip_set_e(&self.command)
    }

    pub fn stdout(&self) -> &CapturedStream {
        &self.stdout
    }

    pub fn stderr(&self) -> &CapturedStream {
        &self.stderr
    }

    pub fn to_wire(&self) -> String {
        let wire = WireFailure {
            line: self.line,
            exit_code: self.exit_code,
            command: self.command.clone(),
            stdout: self.stdout.as_str().to_owned(),
            stderr: self.stderr.as_str().to_owned(),
        };
        format!(
            "{MARKER}{}",
            serde_json::to_string(&wire).expect("command failure fields are JSON serializable")
        )
    }

    pub fn from_wire(message: &str) -> Option<Self> {
        let json = message.split_once(MARKER)?.1;
        let mut values = serde_json::Deserializer::from_str(json).into_iter::<WireFailure>();
        let wire = values.next()?.ok()?;
        Some(Self::new(
            wire.line,
            wire.exit_code,
            wire.command,
            CapturedStream::from_bytes(wire.stdout.as_bytes()),
            CapturedStream::from_bytes(wire.stderr.as_bytes()),
        ))
    }
}

#[cfg(test)]
#[path = "tests/command_failure_tests.rs"]
mod tests;
