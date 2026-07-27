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

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub fn command(&self) -> &str {
        &self.command
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
