use super::*;

/// The call is qualified with the `cook.` receiver and takes its name from
/// the constant, never a re-spelling of it.
#[test]
fn a_door_call_is_qualified_and_takes_one_string_argument() {
    assert_eq!(
        door_call("load_module", "greet"),
        "cook.load_module(\"greet\")"
    );
    assert_eq!(
        door_call(PROBE_SUBST_NAME, "$<k>"),
        "cook.__probe_subst(\"$<k>\")"
    );
}

/// The argument goes through the one Lua string-literal law (COOK-398), so a
/// quote or a carriage return in an ident cannot end the literal or break the
/// generated chunk.
#[test]
fn a_door_call_escapes_its_argument() {
    assert_eq!(door_call("d", "say \"hi\""), "cook.d(\"say \\\"hi\\\"\")");
    assert_eq!(door_call("d", "a\rb"), "cook.d(\"a\\rb\")");
}

#[test]
fn the_probe_subst_call_names_the_installed_helper() {
    assert!(probe_subst_call("$<k>").contains(PROBE_SUBST_NAME));
    assert_eq!(
        probe_subst_call("$<k:field>"),
        "cook.__probe_subst(\"$<k:field>\")"
    );
}

// ------------------------------------------------ the two VMs' door names

/// Every door this module names is a distinct name.
///
/// Not a tautology dressed as a test: the point of one home is that a reader
/// can see the whole surface at once, and a copy-paste that gave two doors the
/// same constant value would install one over the other on both VMs — the
/// later `cook.set` silently wins and the earlier door is simply gone. Nothing
/// else in the workspace looks at these as a set, so nothing else can notice.
#[test]
fn no_two_doors_share_a_name() {
    let doors = [
        ADD_UNIT_NAME,
        STEP_GROUP_NAME,
        PRIOR_OUTPUTS_NAME,
        INTERACTIVE_NAME,
        DEP_OUTPUT_NAME,
        DEP_OUTPUT_LIST_NAME,
        DEP_OUTPUT_MEMBER_NAME,
        MEMBER_TO_STRING_NAME,
        PROBE_SUBST_NAME,
        QUOTE_PARAM_NAME,
        REGISTER_SURFACE_NAME,
        REGISTER_SURFACE_CHORE_NAME,
        MAIN_PROGRAM_NAME,
        CONFIG_DISPATCH_NAME,
    ];
    let unique: std::collections::BTreeSet<&str> = doors.iter().copied().collect();
    assert_eq!(
        unique.len(),
        doors.len(),
        "two doors share a name: {doors:?}"
    );
}

/// `dep_output_list` is not `dep_output` with a suffix by accident: an emitter
/// that composed one from the other would send the plural call to the singular
/// door on any rename. Pinned because `door_call` makes composing them cheap.
#[test]
fn a_door_call_composes_each_name_verbatim() {
    assert_eq!(
        door_call(DEP_OUTPUT_NAME, "lib"),
        "cook.dep_output(\"lib\")"
    );
    assert_eq!(
        door_call(DEP_OUTPUT_LIST_NAME, "lib"),
        "cook.dep_output_list(\"lib\")"
    );
    assert_eq!(
        door_call(MEMBER_TO_STRING_NAME, "x"),
        "cook.member_to_string(\"x\")"
    );
}

/// One condition, one sentence, both phases (§24.7).
///
/// The two VMs wrote this twice in spellings the constitution's
/// duplicate-literal rule cannot pair — `"recipe '{}' has …"` against
/// `"recipe '{name}' has …"` — so this is the only thing standing between the
/// two phases and two different answers to the same mistake.
#[test]
fn the_unknown_referent_diagnostic_is_one_sentence() {
    assert_eq!(
        no_terminal_output_message("build"),
        "recipe 'build' has no terminal output (not registered or has no cook steps)"
    );
}
