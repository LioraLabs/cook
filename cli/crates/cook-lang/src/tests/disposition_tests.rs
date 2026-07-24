use super::*;

#[test]
fn mods_empty_tail_is_default() {
    let m = parse_cook_modifiers("", 1).unwrap();
    assert!(m.seal.is_empty() && m.unseal.is_empty());
    assert_eq!(m.sharing, cook_contracts::Sharing::Shared);
    assert!(!m.record);
}

#[test]
fn mods_share_mod_local_pinned_nondet() {
    assert_eq!(
        parse_cook_modifiers("local", 1).unwrap().sharing,
        cook_contracts::Sharing::Local
    );
    assert_eq!(
        parse_cook_modifiers("pinned", 1).unwrap().sharing,
        cook_contracts::Sharing::Pinned
    );
    assert!(parse_cook_modifiers("nondet", 1).unwrap().record);
}

#[test]
fn mods_seal_unseal_collect_refs() {
    let m = parse_cook_modifiers("seal a b unseal c", 1).unwrap();
    assert_eq!(
        m.seal.iter().cloned().collect::<Vec<_>>(),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        m.unseal.iter().cloned().collect::<Vec<_>>(),
        vec!["c".to_string()]
    );
}

#[test]
fn mods_seal_then_share_mod() {
    let m = parse_cook_modifiers("seal rev local", 1).unwrap();
    assert!(m.seal.contains("rev"));
    assert_eq!(m.sharing, cook_contracts::Sharing::Local);
}

#[test]
fn mods_bare_seal_rejected() {
    assert!(parse_cook_modifiers("seal", 1).is_err());
    // `local` terminates the ref run → bare seal
    assert!(parse_cook_modifiers("seal local", 1).is_err());
    assert!(parse_cook_modifiers("unseal", 1).is_err());
}

#[test]
fn mods_two_share_mods_rejected() {
    assert!(parse_cook_modifiers("local pinned", 1).is_err());
    assert!(parse_cook_modifiers("nondet local", 1).is_err());
}

#[test]
fn mods_content_after_share_mod_rejected() {
    assert!(parse_cook_modifiers("local seal a", 1).is_err());
}

#[test]
fn mods_record_keyword_hints_nondet() {
    let e = parse_cook_modifiers("record", 1).unwrap_err();
    if let ParseError::Parse { message, .. } = e {
        assert!(message.contains("nondet"));
    } else {
        panic!("expected Parse error");
        }
    }

    #[test]
    fn mods_as_keyword_hints_removed_in_v1() {
        let e = parse_cook_modifiers("as 'x'", 1).unwrap_err();
    if let ParseError::Parse { message, .. } = e {
        assert!(message.contains("removed in v1.0"));
    } else {
        panic!("expected Parse error");
        }
    }

    #[test]
    fn mods_seal_quoted_and_triple_colon_rejected() {
        assert!(parse_cook_modifiers("seal \"host\"", 1).is_err());
    assert!(parse_cook_modifiers("seal a:b:c", 1).is_err());
}

#[test]
fn parse_seal_refs_accepts_bare_keys() {
    let refs = vec!["host".to_string(), "cc:toolchain".to_string()];
    let out = parse_seal_refs(&refs, 3).expect("should accept bare keys");
    assert_eq!(out, vec!["host".to_string(), "cc:toolchain".to_string()]);
    let refs = vec!["_x".to_string(), "a1:_b2".to_string()];
    let out = parse_seal_refs(&refs, 3).expect("should accept underscore/digit idents");
    assert_eq!(out, vec!["_x".to_string(), "a1:_b2".to_string()]);
}

#[test]
fn parse_seal_refs_rejects_third_segment() {
    let refs = vec!["a:b:c".to_string()];
    let err = parse_seal_refs(&refs, 7).unwrap_err();
    match err {
        ParseError::Parse { line, message } => {
            assert_eq!(line, 7);
            assert!(message.contains("a:b:c"));
        }
        _ => panic!("expected Parse error"),
        }
    }

    #[test]
    fn parse_seal_refs_rejects_quoted_form() {
        let refs = vec!["\"host\"".to_string()];
    let err = parse_seal_refs(&refs, 2).unwrap_err();
    match err {
        ParseError::Parse { line, message } => {
            assert_eq!(line, 2);
            assert!(message.contains("quoted"));
        }
        _ => panic!("expected Parse error"),
        }
    }

    #[test]
    fn parse_seal_refs_rejects_leading_digit() {
        let refs = vec!["1bad".to_string()];
    assert!(parse_seal_refs(&refs, 1).is_err());
}

#[test]
fn parse_seal_refs_rejects_empty_segment() {
    assert!(parse_seal_refs(&["".to_string()], 1).is_err());
    assert!(parse_seal_refs(&["a:".to_string()], 1).is_err());
    assert!(parse_seal_refs(&[":b".to_string()], 1).is_err());
}
