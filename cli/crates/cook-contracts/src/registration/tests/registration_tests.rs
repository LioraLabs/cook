use super::*;

/// The call is qualified with the `cook.` receiver and takes its name from
/// the constant, never a re-spelling of it.
#[test]
fn a_door_call_is_qualified_and_takes_one_string_argument() {
    assert_eq!(door_call("load_module", "greet"), "cook.load_module(\"greet\")");
    assert_eq!(door_call(PROBE_SUBST_NAME, "$<k>"), "cook.__probe_subst(\"$<k>\")");
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
    assert_eq!(probe_subst_call("$<k:field>"), "cook.__probe_subst(\"$<k:field>\")");
}
