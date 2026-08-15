//! Every `cook.<door>` name this crate writes is either a shared constant or
//! a listed exception (COOK-439).
//!
//! This crate is the EMITTER half of a pair whose other half lives in the two
//! Lua hosts: `cook-luagen` writes `cook.add_unit{…}` into the generated
//! program, `cook-register` installs the function that call finds, and
//! `cook-execute` installs the §6.3.2 guard that refuses it. Drift between
//! emitter and installer is silent. The call resolves to `nil` and the
//! generated program dies at run time with a diagnostic that names neither the
//! rename nor the file that made it.
//!
//! COOK-439 gave those names one home in `cook_contracts::registration`, and
//! this crate reads it wherever the emission is a call it COMPOSES. What it
//! could not read it from is the multi-line `format!` templates, where the
//! door name sits inside a literal several lines long: interpolating a
//! constant into each one costs more legibility than it buys, and the
//! constitution's duplicate-literal rule cannot see a name buried in a literal
//! that size anyway.
//!
//! That leaves a deliberate copy, and the deliberate-copy protocol
//! (`cli/crates/cook-contracts/README.md`) permits one only with an agreement
//! test standing in for the missing edge. This is that test. It reads this
//! crate's own source — the same idiom the constitution gate uses — collects
//! every `cook.<ident>` it names, and requires each to be a name
//! `cook-contracts` declares or an exception listed below with a reason.
//!
//! What it catches: a door renamed in `cook_contracts::registration` while a
//! template kept the old spelling. The constant moves, the literal does not,
//! and the literal is no longer in the shared set. What it does not catch is a
//! door renamed in BOTH places but not in the VM that installs it — nothing
//! short of running the program catches that, which is what
//! `cook-engine/tests/both_phase_door_agreement.rs` is for.

use std::collections::BTreeSet;

/// `cook.<name>` spellings this crate writes that `cook-contracts` does not
/// declare, each with the reason it is not shared.
///
/// A name leaves this list when it gets a constant; nothing may join it
/// without a reason written here, which is the same bargain the constitution's
/// waiver files strike.
const UNSHARED: &[(&str, &str)] = &[
    // Register-phase-only doors: one emitter here, one installer in
    // `cook-register`, and no execute-phase counterpart to disagree with. They
    // were not part of COOK-439's finding, which was about the doors that
    // exist in BOTH phases. They are eligible for the same treatment and
    // nothing has needed it yet.
    ("chore", "register-only chore declaration"),
    ("_enter_chore", "register-only chore window"),
    ("_exit_chore", "register-only chore window"),
    ("recipe", "register-only dynamic registration"),
    ("probe", "register-only probe declaration"),
    ("passthrough", "register-only output passthrough (§5.4.1)"),
    (
        "require_var",
        "register-only declared-variable read (§5.3.1)",
    ),
    ("resolve_gather", "register-only glob resolution"),
    // Both-phase doors this crate names in PROSE only — it does not emit
    // them, so it is not an end of the pair.
    ("sh", "named in doc comments; never emitted by this crate"),
    ("json_decode", "named in doc comments; never emitted"),
    ("probes", "a table, not a door; reached as cook.probes.get"),
    (
        "env",
        "the pre-CS-0172 namespace, named only in historical comments",
    ),
];

/// Every `cook.<ident>` spelling that appears anywhere in this crate's source.
fn door_names_this_crate_writes() -> BTreeSet<String> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeSet::new();
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read cook-luagen src") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                // Test bodies name doors in expectations; they are the thing
                // being checked against, not another emitter.
                if path.file_name().is_none_or(|name| name != "tests") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            let bytes = text.as_bytes();
            let mut i = 0;
            while let Some(at) = text[i..].find("cook.") {
                let start = i + at;
                i = start + "cook.".len();
                // `x.cook.y` and `_cook.y` are not the receiver.
                if start > 0 && {
                    let prev = bytes[start - 1];
                    prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.'
                } {
                    continue;
                }
                let mut end = i;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                if end > i {
                    found.insert(text[i..end].to_string());
                }
            }
        }
    }
    found
}

/// The door names `cook-contracts` declares, which is the set this crate is
/// allowed to write without listing an exception.
fn shared_door_names() -> BTreeSet<&'static str> {
    use cook_contracts::registration::*;
    BTreeSet::from([
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
        cook_contracts::module_binding::LOAD_MODULE_FN,
    ])
}

#[test]
fn every_door_this_crate_writes_is_shared_or_listed() {
    let shared = shared_door_names();
    let listed: BTreeSet<&str> = UNSHARED.iter().map(|(name, _)| *name).collect();

    let unexplained: Vec<String> = door_names_this_crate_writes()
        .into_iter()
        .filter(|name| !shared.contains(name.as_str()) && !listed.contains(name.as_str()))
        .collect();

    assert!(
        unexplained.is_empty(),
        "cook-luagen writes these `cook.<door>` names, and cook-contracts declares none of \
         them: {unexplained:?}.\n\
         Either the door was renamed in cook_contracts::registration and a template here kept \
         the old spelling — which is emitter/installer drift that fails only at run time, as a \
         call to a nil value — or a new door was added and needs a constant, or it is genuinely \
         unshared and belongs in this file's UNSHARED list with a reason."
    );
}

/// The exception list shrinks or stays honest; it does not accumulate names
/// that have since been shared.
#[test]
fn no_listed_exception_is_already_shared() {
    let shared = shared_door_names();
    let stale: Vec<&str> = UNSHARED
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| shared.contains(name))
        .collect();
    assert!(
        stale.is_empty(),
        "these names are listed as unshared but cook-contracts declares them: {stale:?}. \
         Delete the lines — a list that only grows stops being read."
    );
}

/// And it does not accumulate names this crate has stopped writing.
#[test]
fn no_listed_exception_names_a_door_this_crate_no_longer_writes() {
    let written = door_names_this_crate_writes();
    let gone: Vec<&str> = UNSHARED
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !written.contains(*name))
        .collect();
    assert!(gone.is_empty(), "listed but no longer written: {gone:?}");
}
