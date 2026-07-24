use super::*;

#[test]
fn recipe_id_round_trips_through_eq_and_hash() {
    let a = RecipeId::new(0);
    let b = RecipeId::new(0);
    let c = RecipeId::new(1);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn progress_event_is_clone_and_send() {
    fn assert_clone_send<T: Clone + Send>() {}
    assert_clone_send::<ProgressEvent>();
}

#[test]
fn skip_reason_display() {
    assert_eq!(SkipReason::UpstreamFailed.as_str(), "upstream-failed");
    assert_eq!(SkipReason::ConditionFalse.as_str(), "condition-false");
    assert_eq!(SkipReason::Disabled.as_str(), "disabled");
}
