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
fn parse_seal_refs_accepts_bare_keys() {
    let refs = vec!["host".to_string(), "cc:toolchain".to_string()];
    let out = parse_seal_refs(&refs, 3).expect("should accept bare keys");
    assert_eq!(out, vec!["host".to_string(), "cc:toolchain".to_string()]);
    let refs = vec!["_x".to_string(), "a1:_b2".to_string()];
    let out = parse_seal_refs(&refs, 3).expect("should accept underscore/digit idents");
    assert_eq!(out, vec!["_x".to_string(), "a1:_b2".to_string()]);
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

/// CS-0201: `seal` takes both spellings a probe key has. It previously took
/// neither `-` nor `.`, capped at two segments, and refused the quoted form
/// outright — while the declaration that mints the key allowed `-`, `.` and
/// quoting. `probe cc-version` therefore produced a key that could not be
/// sealed, and `cc:find:raylib` could not be sealed either, which is exactly
/// the pin a cache-trust story exists to offer.
#[test]
fn parse_seal_refs_takes_hyphens_multi_segments_and_the_quoted_form() {
    let got = parse_seal_refs(
        &[
            "host".to_string(),
            "cc-version".to_string(),
            "demo:cc-version".to_string(),
            "cc:find:raylib".to_string(),
            "\"any spelling+here\"".to_string(),
        ],
        1,
    )
    .expect("all five are valid probe key refs");
    assert_eq!(
        got,
        vec![
            "host".to_string(),
            "cc-version".to_string(),
            "demo:cc-version".to_string(),
            "cc:find:raylib".to_string(),
            "any spelling+here".to_string(),
        ]
    );
}

#[test]
fn parse_seal_refs_rejects_a_dotted_bare_key_and_says_why() {
    let err = parse_seal_refs(&["cc.version".to_string()], 3).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("member access"), "must explain the dot: {msg}");
    assert!(msg.contains("quoted"), "must offer the escape hatch: {msg}");
}

#[test]
fn parse_seal_refs_rejects_an_empty_quoted_key() {
    assert!(parse_seal_refs(&["\"\"".to_string()], 1).is_err());
}
