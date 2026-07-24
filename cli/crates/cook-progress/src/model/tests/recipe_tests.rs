use super::*;

#[test]
fn new_recipe_is_waiting_with_zero_progress() {
    let r = RecipeState::new(RecipeId::new(0), "deps".into(), vec![], 12);
    assert_eq!(r.status, Status::Waiting);
    assert_eq!(r.progress, (0, 12));
    assert!(r.nodes.is_empty());
    assert_eq!(r.cached_count, 0);
    assert!(r.error_summary.is_none());
}
