use super::hash_str;

#[test]
fn is_stable_for_the_same_input() {
    assert_eq!(hash_str("cc -c main.c"), hash_str("cc -c main.c"));
}

#[test]
fn distinguishes_inputs() {
    assert_ne!(hash_str("a"), hash_str("b"));
    assert_ne!(hash_str(""), hash_str(" "));
}

/// The empty string must hash to something, not to zero-as-sentinel: callers
/// fold this into cache keys and a magic zero would collide with "absent".
#[test]
fn the_empty_string_has_a_hash() {
    assert_ne!(hash_str(""), 0);
}
