use super::*;

// ---------------------------------------------------------------------------
// Local cache key: the identity a unit is filed under (§17.1.1.1, CS-0186)
// ---------------------------------------------------------------------------

/// These call `build_local_cache_key` directly rather than through
/// `cook.add_unit`, because the properties under test are properties of
/// the key composition: driving them through the Lua surface would let a
/// declaration detail decide whether the assertion held.
fn key(outputs: &[&str], inputs: &[&str], command_hash: u64, env: u64) -> String {
    keyed(outputs, inputs, command_hash, env, &[])
}

/// The same, with an effective seal key set — the determinant §17.1.1.1
/// exclusion 3 requires the identity to separate units by.
fn keyed(
    outputs: &[&str],
    inputs: &[&str],
    command_hash: u64,
    env: u64,
    seal: &[&str],
) -> String {
    let outputs: Vec<String> = outputs.iter().map(|s| s.to_string()).collect();
    let inputs: Vec<crate::cache::DeclaredInput> =
        inputs.iter().map(|s| (*s).into()).collect();
    let seal: std::collections::BTreeSet<String> =
        seal.iter().map(|s| s.to_string()).collect();
    build_local_cache_key("Cookfile", "r", &outputs, &inputs, command_hash, env, &seal)
}

#[test]
fn a_producing_unit_is_identified_by_its_first_output() {
    assert_eq!(key(&["a.o", "a.d"], &["a.c"], 0xbeef, 0), "a.o");
    assert_eq!(key(&["a.o"], &["a.c"], 0xbeef, 0x1234), "a.o@1234");
}

/// The producing branch reads NOTHING but the output list and the env
/// contribution. Stated as a test because CS-0186 restructured this function
/// around a shared `<identity>@<env>` tail, and a cook unit's key moving would
/// invalidate every artifact in every index in the project.
#[test]
fn a_producing_units_key_ignores_inputs_and_command() {
    let a = key(&["a.o"], &["a.c"], 0xbeef, 0);
    let b = key(&["a.o"], &["totally", "different"], 0xfeed, 0);
    assert_eq!(a, b, "a producing key is its output path and nothing else");
}

#[test]
fn an_observing_unit_is_identified_by_a_declaration_digest() {
    let k = key(&[], &["a.c"], 0xbeef, 0);
    assert!(k.starts_with(OBSERVING_KEY_MARKER), "observing keys carry the marker: {k}");
    assert_eq!(k.len(), 17, "marker plus 16 hex digits: {k}");
    assert!(k[1..].chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn an_observing_unit_carries_the_env_contribution_in_the_same_position() {
    let bare = key(&[], &["a.c"], 0xbeef, 0);
    let with_env = key(&[], &["a.c"], 0xbeef, 0x1234);
    assert_eq!(with_env, format!("{bare}@1234"));
}

/// The marker is what keeps the two identity spaces apart in one index. A
/// declared output path is workspace-relative and never opens with it.
#[test]
fn the_two_identity_spaces_do_not_overlap() {
    let producing = key(&["build/demo"], &["a.c"], 0xbeef, 0);
    let observing = key(&[], &["a.c"], 0xbeef, 0);
    assert!(!producing.starts_with(OBSERVING_KEY_MARKER));
    assert!(observing.starts_with(OBSERVING_KEY_MARKER));
    assert_ne!(producing, observing);
}

// --- what the old `<first-input>@<command-hash>` form could not tell apart ---

/// The defect that made the old form unsafe to rely on: two units sharing a
/// first input and a command text were ONE entry, because nothing past the
/// first input reached the key.
#[test]
fn units_differing_only_past_the_first_input_are_distinct() {
    let a = key(&[], &["shared.c", "one.c"], 0xbeef, 0);
    let b = key(&[], &["shared.c", "two.c"], 0xbeef, 0);
    assert_ne!(a, b, "every declared input reaches the identity, not just the first");
}

#[test]
fn units_differing_only_in_input_count_are_distinct() {
    assert_ne!(key(&[], &["a.c"], 0xbeef, 0), key(&[], &["a.c", "b.c"], 0xbeef, 0));
}

/// A unit declaring no inputs at all keyed as `@<command-hash>` under the old
/// form — the empty string plus a hash — so two such units in one recipe
/// collided unless their command text differed.
#[test]
fn units_declaring_no_inputs_are_still_told_apart_by_command() {
    let a = key(&[], &[], 0xbeef, 0);
    let b = key(&[], &[], 0xfeed, 0);
    assert_ne!(a, b);
    assert!(a.starts_with(OBSERVING_KEY_MARKER) && b.starts_with(OBSERVING_KEY_MARKER));
}

#[test]
fn units_differing_only_in_command_are_distinct() {
    assert_ne!(key(&[], &["a.c"], 0xbeef, 0), key(&[], &["a.c"], 0xfeed, 0));
}

/// Path boundaries are unambiguous: the concatenation of one input list must
/// not hash like the concatenation of a different one.
#[test]
fn input_paths_cannot_run_together() {
    assert_ne!(key(&[], &["ab", "c"], 0xbeef, 0), key(&[], &["a", "bc"], 0xbeef, 0));
}

// --- what the identity deliberately does NOT depend on ---

/// The exclusion the whole design rests on. If contents reached the identity,
/// no record would ever be found twice and every invalidation would present as
/// a first-ever build with no reportable cause. The key is a function of the
/// declaration alone, and these arguments carry no content.
#[test]
fn the_identity_is_stable_across_everything_but_the_declaration() {
    let first = key(&[], &["a.c", "b.c"], 0xbeef, 0x1234);
    let again = key(&[], &["a.c", "b.c"], 0xbeef, 0x1234);
    assert_eq!(first, again, "the identity is a pure function of the declaration");
}

/// §17.4: moving a test within a recipe, or a recipe between Cookfiles, MUST
/// NOT bust its cache. Both are arguments to this function and both are unused.
#[test]
fn neither_the_recipe_nor_the_cookfile_reaches_the_identity() {
    let outputs: Vec<String> = vec![];
    let inputs: Vec<crate::cache::DeclaredInput> = vec!["a.c".into()];
    let seal = std::collections::BTreeSet::new();
    let here =
        build_local_cache_key("Cookfile", "check", &outputs, &inputs, 0xbeef, 0, &seal);
    let moved =
        build_local_cache_key("sub/Cookfile", "verify", &outputs, &inputs, 0xbeef, 0, &seal);
    assert_eq!(here, moved);
}

/// A path reached by two routes — named by `inputs` and again by a step-group
/// dep — must not change what the unit IS. The test payload's input list has
/// deduplicated since COOK-84; the identity now does too.
#[test]
fn reaching_one_path_twice_does_not_change_the_identity() {
    assert_eq!(
        key(&[], &["a.c", "b.c"], 0xbeef, 0),
        key(&[], &["a.c", "b.c", "a.c"], 0xbeef, 0)
    );
}

/// Order is kept, because a reordering IS a change to the declaration. Stated
/// so that a later switch to sorting is a deliberate decision rather than a
/// silent one.
#[test]
fn input_order_is_part_of_the_declaration() {
    assert_ne!(key(&[], &["a.c", "b.c"], 0xbeef, 0), key(&[], &["b.c", "a.c"], 0xbeef, 0));
}

/// The effective seal key set separates two units that declare nothing else
/// differently.
///
/// The `keyed` helper above has advertised this since the tests were written
/// and no case used it: every call came through `key(…)`, which passes an
/// empty set. So the determinant with the longest paragraph in
/// `observing_identity`'s doc — and the whole defence against the CS-0169
/// churn shape §17.1.1.1 exclusion 3 describes — was documented, argued, and
/// unasserted. Found on the move (COOK-421), because a move is when you
/// re-read what you are carrying.
#[test]
fn the_effective_seal_key_set_separates_two_otherwise_identical_units() {
    let bare = keyed(&[], &["run.sh"], 0xbeef, 0, &[]);
    let sealed = keyed(&[], &["run.sh"], 0xbeef, 0, &["toolchain"]);
    assert_ne!(
        bare, sealed,
        "`test {{ ./run }} seal toolchain` and a bare `test {{ ./run }}` are two units"
    );
}

/// Sorted by the `BTreeSet` they arrive in, so declaring the same seals in a
/// different order is the same declaration.
#[test]
fn seal_key_order_is_not_part_of_the_identity() {
    assert_eq!(
        keyed(&[], &["run.sh"], 0xbeef, 0, &["a", "b"]),
        keyed(&[], &["run.sh"], 0xbeef, 0, &["b", "a"])
    );
}

/// Different seal sets are different identities, including one that is a
/// subset of the other.
#[test]
fn adding_a_seal_key_moves_the_identity() {
    let one = keyed(&[], &["run.sh"], 0xbeef, 0, &["a"]);
    let two = keyed(&[], &["run.sh"], 0xbeef, 0, &["a", "b"]);
    assert_ne!(one, two);
}

/// The observing identity as exact bytes.
///
/// Every other test here is relational — these two differ, those two agree —
/// so the whole digest could move and stay green. It is a LOCAL index key
/// rather than a cross-machine one, but a silent change to it misses every
/// test unit in every project at once, which is the same question
/// `context_tests` asks of the probe fingerprint: not "which assertion do I
/// update" but "did I mean to invalidate the world".
///
/// Computed with an independent xxh3 (Python's `xxhash`) rather than read off
/// a passing run. The preimage, so it stays checkable: xxh3-64
/// over `b"a.c\0=" b"b.c\0=" 0xbeefu64.to_le_bytes() b"toolchain\0"`,
/// rendered as the marker `:` and sixteen lower-hex digits.
#[test]
fn the_observing_identity_is_these_exact_bytes() {
    assert_eq!(keyed(&[], &["a.c", "b.c"], 0xbeef, 0, &["toolchain"]), ":cee58b942fa4c6ff");
}
