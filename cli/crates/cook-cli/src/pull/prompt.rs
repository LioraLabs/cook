//! Conflict prompting abstraction. Production code uses [`StdinPrompter`];
//! tests use [`ScriptedPrompter`].

use std::io::{BufRead, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAnswer {
    Yes,
    No,
    All,
    Quit,
}

/// Asks the user what to do about a conflicting file. The caller invokes
/// `prompt` once per conflicting path. When an implementation returns `All`,
/// it MAY short-circuit subsequent calls and return `Yes` without further
/// I/O — `StdinPrompter` does this. Callers that want fresh prompts after
/// `All` should construct a new prompter.
pub trait ConflictPrompter {
    fn prompt(&mut self, path: &Path) -> ConflictAnswer;
}

/// Production implementation backed by stdin / stderr.
pub struct StdinPrompter<R: BufRead, W: Write> {
    stdin: R,
    stderr: W,
    all_yes_sticky: bool,
}

impl<R: BufRead, W: Write> StdinPrompter<R, W> {
    pub fn new(stdin: R, stderr: W) -> Self {
        Self {
            stdin,
            stderr,
            all_yes_sticky: false,
        }
    }
}

impl<R: BufRead, W: Write> ConflictPrompter for StdinPrompter<R, W> {
    fn prompt(&mut self, path: &Path) -> ConflictAnswer {
        if self.all_yes_sticky {
            return ConflictAnswer::Yes;
        }
        loop {
            let _ = write!(
                self.stderr,
                "overwrite {}? [y/N/a=all/q=quit]: ",
                path.display()
            );
            let _ = self.stderr.flush();
            let mut line = String::new();
            match self.stdin.read_line(&mut line) {
                Ok(0) | Err(_) => return ConflictAnswer::No, // EOF or read error → safe default
                Ok(_) => {}
            }
            match line.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return ConflictAnswer::Yes,
                "n" | "no" | "" => return ConflictAnswer::No,
                "a" | "all" => {
                    self.all_yes_sticky = true;
                    return ConflictAnswer::All;
                }
                "q" | "quit" => return ConflictAnswer::Quit,
                _ => continue, // re-prompt
            }
        }
    }
}

/// Test implementation: scripted answers, one per call. Panics if exhausted.
pub struct ScriptedPrompter {
    answers: std::vec::IntoIter<ConflictAnswer>,
    pub asked: Vec<std::path::PathBuf>,
}

impl ScriptedPrompter {
    pub fn new(answers: Vec<ConflictAnswer>) -> Self {
        Self {
            answers: answers.into_iter(),
            asked: Vec::new(),
        }
    }
}

impl ConflictPrompter for ScriptedPrompter {
    fn prompt(&mut self, path: &Path) -> ConflictAnswer {
        self.asked.push(path.to_path_buf());
        self.answers
            .next()
            .expect("ScriptedPrompter exhausted: install asked more times than scripted")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;

    #[test]
    fn scripted_returns_in_order() {
        let mut p = ScriptedPrompter::new(vec![
            ConflictAnswer::Yes,
            ConflictAnswer::No,
            ConflictAnswer::Quit,
        ]);
        assert_eq!(p.prompt(Path::new("a")), ConflictAnswer::Yes);
        assert_eq!(p.prompt(Path::new("b")), ConflictAnswer::No);
        assert_eq!(p.prompt(Path::new("c")), ConflictAnswer::Quit);
        assert_eq!(
            p.asked,
            vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
        );
    }

    #[test]
    #[should_panic]
    fn scripted_panics_when_exhausted() {
        let mut p = ScriptedPrompter::new(vec![]);
        let _ = p.prompt(Path::new("x"));
    }

    #[test]
    fn stdin_yes_then_no() {
        let stdin = Cursor::new(b"y\nn\n");
        let mut stderr = Vec::new();
        let mut p = StdinPrompter::new(stdin, &mut stderr);
        assert_eq!(p.prompt(Path::new("a")), ConflictAnswer::Yes);
        assert_eq!(p.prompt(Path::new("b")), ConflictAnswer::No);
    }

    #[test]
    fn stdin_empty_line_is_no() {
        let stdin = Cursor::new(b"\n");
        let mut stderr = Vec::new();
        let mut p = StdinPrompter::new(stdin, &mut stderr);
        assert_eq!(p.prompt(Path::new("a")), ConflictAnswer::No);
    }

    #[test]
    fn stdin_eof_is_no() {
        let stdin = Cursor::new(b"");
        let mut stderr = Vec::new();
        let mut p = StdinPrompter::new(stdin, &mut stderr);
        assert_eq!(p.prompt(Path::new("a")), ConflictAnswer::No);
    }

    #[test]
    fn stdin_all_short_circuits_subsequent() {
        let stdin = Cursor::new(b"a\n");
        let mut stderr = Vec::new();
        let mut p = StdinPrompter::new(stdin, &mut stderr);
        assert_eq!(p.prompt(Path::new("a")), ConflictAnswer::All);
        // No more input on stdin, but subsequent calls should not read it.
        assert_eq!(p.prompt(Path::new("b")), ConflictAnswer::Yes);
        assert_eq!(p.prompt(Path::new("c")), ConflictAnswer::Yes);
    }

    #[test]
    fn stdin_quit_returned() {
        let stdin = Cursor::new(b"q\n");
        let mut stderr = Vec::new();
        let mut p = StdinPrompter::new(stdin, &mut stderr);
        assert_eq!(p.prompt(Path::new("a")), ConflictAnswer::Quit);
    }

    #[test]
    fn stdin_invalid_input_re_prompts() {
        let stdin = Cursor::new(b"maybe\nyes\n");
        let mut stderr = Vec::new();
        let mut p = StdinPrompter::new(stdin, &mut stderr);
        assert_eq!(p.prompt(Path::new("a")), ConflictAnswer::Yes);
    }
}
