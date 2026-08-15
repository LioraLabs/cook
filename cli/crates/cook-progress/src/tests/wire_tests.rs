use super::*;
use crate::event::{NodeKind, SkipReason, Stream};

#[test]
fn a_line_round_trips_through_the_one_schema() {
    let line = WireLine {
        ts: "2026-07-31T12:00:00Z".to_string(),
        v: 1,
        event: WireEvent::NodeSkipped {
            recipe: "deps".into(),
            node: "lvm.c".into(),
            reason: SkipReason::ConditionFalse,
        },
    };
    let json = serde_json::to_string(&serde_json::to_value(&line).unwrap()).unwrap();
    let back: WireLine = serde_json::from_str(&json).unwrap();
    assert_eq!(back, line);
}

#[test]
fn serialized_bytes_match_the_historical_hand_built_shape() {
    // Sorted keys, kebab-case tag, enum spellings from the derives — the
    // exact line the hand-built writer produced before COOK-394.
    let line = WireLine {
        ts: "2026-07-31T12:00:00Z".to_string(),
        v: 1,
        event: WireEvent::NodeOutput {
            recipe: "lib".into(),
            node: "lvm.c".into(),
            stream: Stream::Stderr,
            line: "warning: unused".into(),
        },
    };
    let json = serde_json::to_string(&serde_json::to_value(&line).unwrap()).unwrap();
    assert_eq!(
        json,
        r#"{"line":"warning: unused","node":"lvm.c","recipe":"lib","stream":"stderr","ts":"2026-07-31T12:00:00Z","type":"node-output","v":1}"#
    );
}

#[test]
fn node_started_kind_spellings_come_from_the_derive() {
    for (kind, tag) in [
        (NodeKind::Compile, "\"kind\":\"compile\""),
        (NodeKind::Link, "\"kind\":\"link\""),
        (NodeKind::Cooked, "\"kind\":\"cooked\""),
    ] {
        let line = WireLine {
            ts: "t".into(),
            v: 1,
            event: WireEvent::NodeStarted {
                recipe: "r".into(),
                node: "n".into(),
                artifact: None,
                fallback_label: "$ cc".into(),
                kind,
                cause: None,
                cache_key: None,
            },
        };
        let json = serde_json::to_string(&serde_json::to_value(&line).unwrap()).unwrap();
        assert!(json.contains(tag), "expected {tag} in {json}");
    }
}

#[test]
fn unknown_reader_defaults_are_gone_not_silent() {
    // An unknown node kind used to silently become Cooked; under the one
    // schema it is a parse failure the caller counts/skips deliberately.
    let bad = r#"{"artifact":null,"cache_key":null,"cause":null,"fallback_label":"x","kind":"warp-drive","node":"n","recipe":"r","ts":"t","type":"node-started","v":1}"#;
    assert!(serde_json::from_str::<WireLine>(bad).is_err());
    // Unknown stream likewise.
    let bad = r#"{"line":"x","node":"n","recipe":"r","stream":"sideband","ts":"t","type":"node-output","v":1}"#;
    assert!(serde_json::from_str::<WireLine>(bad).is_err());
}

#[test]
fn additive_fields_are_ignored_and_defaults_fill_absent_ones() {
    // Additive-without-bump evolution: a NEWER writer's extra field parses;
    // an OLDER writer's absent #[serde(default)] field parses.
    let newer = r#"{"future_field":42,"recipe":"deps","ts":"t","type":"recipe-started","v":1}"#;
    assert!(serde_json::from_str::<WireLine>(newer).is_ok());
    let older = r#"{"cached":1,"elapsed_ms":10,"recipe":"deps","total":2,"ts":"t","type":"recipe-completed","v":1}"#;
    let line: WireLine = serde_json::from_str(older).unwrap();
    assert!(matches!(
        line.event,
        WireEvent::RecipeCompleted { kind: crate::event::RecipeKind::Recipe, .. }
    ));
}
