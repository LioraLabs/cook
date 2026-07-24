use super::*;
#[test]
fn test_result_carries_line() {
    let r = TestResult {
        id: TestId("r:t".into()),
        namespace: String::new(),
        recipe: "r".into(),
            name: "t".into(),
        suite: String::new(),
        iteration_item: None,
        outcome: TestOutcome::Passed,
        duration: std::time::Duration::ZERO,
        from_cache: false,
        stdout: String::new(),
        stderr: String::new(),
        fingerprint: None,
        blocked_by: None,
        should_fail: false,
        timed_out: false,
        line: 42,
        exit_code: None,
    };
    assert_eq!(r.line, 42);
}

#[test]
fn test_started_event_carries_line() {
    let evt = EngineEvent::TestStarted {
        id: TestId("r:t".into()),
        recipe: "r".into(),
            name: "t".into(),
        line: 7,
        iteration_item: None,
    };
    if let EngineEvent::TestStarted { line, .. } = evt {
        assert_eq!(line, 7);
    } else {
        panic!("wrong variant");
    }
}
