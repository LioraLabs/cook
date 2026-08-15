//! COOK-439: a door the Standard marks **Phase: Both** must answer the same
//! question the same way in both phases.
//!
//! Cook runs two Lua hosts. `cook-register` owns the register-phase VM;
//! `cook-execute` owns the worker VMs that evaluate `>{ … }` bodies. §24
//! ("Both-phase API") says a list of doors exists on both — `cook.sh`,
//! `cook.load_module`, `var`, `cook.probes`, `cook.platform`,
//! `cook.dep_output` (§24.7), the codecs (§24.8) — and §9.3 says the same of
//! `cook.member_to_string`. What §24 does NOT do is give either VM a reason to
//! consult the other: each installs its own function into its own `cook` table,
//! and until COOK-439 several of those installs were two separate bodies with
//! two separate spellings of the same intent.
//!
//! That is a drift that cannot fail to compile. Two independent installs of one
//! door type-check individually forever; what changes is what a Cookfile can do.
//! A rendering rule improved on one VM and not the other silently makes `$<in>`
//! and a body's own `cook.member_to_string(item)` disagree about the same value,
//! and a diagnostic reworded on one VM leaves the other phase telling the author
//! something else about an identical mistake. Nothing in the build fails; the
//! two phases just stop being one language.
//!
//! So this file states the agreement where a user can reach it: the same Lua
//! source text, evaluated once by each VM, through the real `cook` binary.
//!
//! **This test is expected to be trivially true, and that is the point.** The
//! constitution's standing agreement-test rule (COOK-361, see
//! `cli/crates/cook-contracts/README.md`) keeps an agreement test alive after
//! the twin implementations have been collapsed into one: once
//! `cook_lua_stdlib::install_member_to_string` and
//! `cook_contracts::registration::no_terminal_output_message` are the single
//! author of each answer, nothing here can fail — until someone re-forks a door
//! for a phase-local reason, at which point this is the thing that notices. A
//! test that only bites while the bug exists is a bug report; this is a guard.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // /target/debug/deps  →  /target/debug
    path.pop(); // /target/debug       →  /target
    path.push("cook");
    assert!(
        path.exists(),
        "cook binary not found at {} — run `cargo build --bin cook` first",
        path.display()
    );
    path
}

/// A workspace with a private cache backend, so no warm shared store can serve
/// a step's output as a first-run hit and leave the worker VM's body — the very
/// thing under test — unevaluated.
fn workspace(cookfile: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    fs::create_dir_all(dir.join(".cook")).unwrap();
    fs::write(
        dir.join(".cook/cloud.toml"),
        format!(
            "[cache]\ncache_dir = {:?}\n",
            dir.join(".cook/shared-cache").to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(dir.join("Cookfile"), cookfile).unwrap();
    tmp
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("cook invocation")
}

fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Re-indent a Lua fragment so the identical source text can be pasted into two
/// Cookfile block bodies that sit at different depths. Lua does not care about
/// leading whitespace; the Cookfile block scanner does, and the point of this
/// file is that the two phases evaluate the SAME text.
fn indent(lua: &str, pad: &str) -> String {
    lua.lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The rendering exercise, written once and run twice.
///
/// A record whose keys are written in non-sorted order (and whose nested record
/// is too) is the case §9.3's canonicalisation exists for: Lua's own table
/// iteration order is unspecified, so a rendering that did not sort would differ
/// run to run, let alone phase to phase. The scalars pin the other half of §9.3
/// — a string renders bare, without the quotes JSON would give it, while a
/// number and a boolean render as their JSON text — and the array pins that a
/// sequence stays a sequence.
///
/// Leaves the result in the local `rendered`; each phase supplies its own sink.
const RENDER_LUA: &str = r#"local record = { zeta = 1, alpha = "two", middle = true, nested = { b = 2, a = { 1, 2, 3 } } }
local parts = {
    cook.member_to_string(record),
    cook.member_to_string("bare string"),
    cook.member_to_string(42),
    cook.member_to_string(true),
    cook.member_to_string({ "x", "y" }),
}
local rendered = table.concat(parts, "\n") .. "\n""#;

/// What §9.3 says both phases owe: compact key-sorted JSON for a table, the raw
/// text for a string scalar, JSON scalar text for a number and a boolean.
const CANONICAL_RENDERING: &str = concat!(
    r#"{"alpha":"two","middle":true,"nested":{"a":[1,2,3],"b":2},"zeta":1}"#,
    "\n",
    "bare string\n",
    "42\n",
    "true\n",
    r#"["x","y"]"#,
    "\n",
);

/// §9.3 / §24: `cook.member_to_string` is a both-phase door, so the same value
/// must render to the same bytes whichever VM is asked.
///
/// The register-phase copy is what `$<in>` and the per-member cache fingerprint
/// are built from (`member = cook.member_to_string(item)` in the generated Lua);
/// the execute-phase copy is what a `>{ … }` body sees when it renders a member
/// itself. A body that reconstructs the string its own unit was keyed under has
/// to get the same answer, or the two are talking about different members.
///
/// The phase-agreement assertion compares the two ACTUAL renderings, so it
/// states "the phases agree" rather than "this is today's format". The canonical
/// assertion that follows is the net under that: a change that broke both phases
/// identically would satisfy agreement and fail here.
#[test]
fn member_to_string_renders_the_same_bytes_in_register_and_execute_phase() {
    let cookfile = format!(
        r#"register
{register_lua}
    fs.write("register.rendering", rendered)

recipe render
    cook "build/execute.rendering" >{{
{execute_lua}
        fs.write(output, rendered)
    }}
"#,
        register_lua = indent(RENDER_LUA, "    "),
        execute_lua = indent(RENDER_LUA, "        "),
    );

    let tmp = workspace(&cookfile);
    let dir = tmp.path();
    let out = run(dir, &["render"]);
    assert!(
        out.status.success(),
        "cook render failed:\n{}",
        combined(&out)
    );

    // The register block writes on every invocation; the recipe body ran cold
    // above, so both files are this run's work.
    let from_register = fs::read_to_string(dir.join("register.rendering"))
        .expect("register-phase rendering: the `register` block must have written it");
    let from_execute = fs::read_to_string(dir.join("build/execute.rendering"))
        .expect("execute-phase rendering: the worker body must have written it");

    assert_eq!(
        from_register, from_execute,
        "cook.member_to_string rendered the same values differently in the two \
         phases. §9.3 and §24 make it one door; a Cookfile that renders a member \
         in a `>{{ … }}` body would disagree with the string its own unit was \
         keyed under (COOK-439)",
    );

    assert_eq!(
        from_register, CANONICAL_RENDERING,
        "both phases agree, but not with §9.3: a table must render as compact \
         key-sorted JSON and a string scalar as its bare text. A regression that \
         moved both phases together would pass the agreement check above and \
         must be caught here",
    );
}

/// §24.7: `cook.dep_output` is a both-phase door, and a reference to a name with
/// no terminal output is an error in both phases. The two phases resolve against
/// different stores — the register VM against the live registration map (where
/// it also records a DAG edge), the worker VM read-only against the closed
/// registration snapshot — but the author made ONE mistake and must be told it
/// in ONE sentence.
///
/// Before COOK-439 that sentence was written twice, positionally in
/// `cook-register` and inline in `cook-execute`. Two spellings of one format
/// string are invisible to the duplicate-literal gate, so improving the wording
/// in the phase you happened to be reading would leave the other phase saying
/// something else, with nothing failing. It now comes from
/// `cook_contracts::registration::no_terminal_output_message`.
///
/// Two workspaces and two runs, because each run is a failure: one Cookfile
/// makes the call at register phase, the other from a worker body.
#[test]
fn dep_output_on_an_unregistered_name_says_the_same_sentence_in_both_phases() {
    const MISSING: &str = "no_such_recipe";
    let expected =
        format!("recipe '{MISSING}' has no terminal output (not registered or has no cook steps)");

    // Register phase: a `register` block is register-phase Lua, and the lookup
    // that fails here is the one `cook.dep_output` performs before anything
    // else it does.
    let register_tmp = workspace(
        r#"register
    local _ = cook.dep_output("no_such_recipe")

recipe build
    cook "build/out.txt" { echo hi > $<out> }
"#,
    );
    let register_run = run(register_tmp.path(), &["build"]);
    assert!(
        !register_run.status.success(),
        "a register-phase cook.dep_output on an unregistered name must fail:\n{}",
        combined(&register_run)
    );

    // Execute phase: the same call from a worker VM's `>{ … }` body.
    let execute_tmp = workspace(
        r#"recipe build
    cook "build/out.txt" >{
        local _ = cook.dep_output("no_such_recipe")
        fs.write(output, "unreachable")
    }
"#,
    );
    let execute_run = run(execute_tmp.path(), &["build"]);
    assert!(
        !execute_run.status.success(),
        "an execute-phase cook.dep_output on an unregistered name must fail:\n{}",
        combined(&execute_run)
    );
    assert!(
        !execute_tmp.path().join("build/out.txt").exists(),
        "the worker body must not have run past the failing call:\n{}",
        combined(&execute_run)
    );

    let register_sentence = sentence_about(&combined(&register_run), MISSING);
    let execute_sentence = sentence_about(&combined(&execute_run), MISSING);

    assert_eq!(
        register_sentence, execute_sentence,
        "the two phases told the author two different things about one mistake \
         (§24.7, COOK-439)",
    );
    assert_eq!(
        register_sentence, expected,
        "both phases agree, but neither says what \
         cook_contracts::registration::no_terminal_output_message says",
    );
}

/// Pull the diagnostic out of a run's output: the tail of the first line that
/// talks about `name`, starting where the sentence starts.
///
/// Each phase wraps the sentence in its own presentation — the register error
/// arrives under a bare `cook:` prefix, the execute one under the failing unit's
/// attribution — and comparing the wrappers would be comparing renderers, not
/// doors. What must match is the sentence itself.
fn sentence_about(output: &str, name: &str) -> String {
    let needle = format!("recipe '{name}'");
    output
        .lines()
        .find_map(|line| {
            line.find(&needle)
                .map(|at| line[at..].trim_end().to_string())
        })
        .unwrap_or_else(|| {
            panic!("no diagnostic mentioning `{needle}` in the run's output:\n{output}")
        })
}
