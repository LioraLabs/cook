use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_load_env_from_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".env"), "FOO=bar\nBAZ=qux\n").unwrap();
    let env = load_env(dir.path());
    assert_eq!(env.get("FOO").unwrap(), "bar");
        assert_eq!(env.get("BAZ").unwrap(), "qux");
}

#[test]
fn test_missing_env_file_returns_empty() {
    let dir = TempDir::new().unwrap();
    let env = load_env(dir.path());
    assert!(env.is_empty());
}

#[test]
fn test_comments_and_blank_lines() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(".env"),
        "# This is a comment\n\nKEY=value\n\n# Another comment\nKEY2=value2\n",
    )
    .unwrap();
    let env = load_env(dir.path());
    assert_eq!(env.len(), 2);
    assert_eq!(env.get("KEY").unwrap(), "value");
    assert_eq!(env.get("KEY2").unwrap(), "value2");
}

#[test]
fn test_quoted_values() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(".env"),
        "SINGLE='hello world'\nDOUBLE=\"hello world\"\n",
    )
    .unwrap();
    let env = load_env(dir.path());
    assert_eq!(env.get("SINGLE").unwrap(), "hello world");
    assert_eq!(env.get("DOUBLE").unwrap(), "hello world");
}

#[test]
fn test_resolve_env_invalid_set() {
    let result = resolve_env(None, HashMap::new(), &["NOT_A_PAIR".to_string()]);
    assert!(matches!(result, Err(PipelineError::InvalidSet(_))));
}
