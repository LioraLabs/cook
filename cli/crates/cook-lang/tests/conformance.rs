//! Conformance corpus harness.
//!
//! Walks `standard/conformance/` and asserts that `cook-lang` parses
//! positive cases into the expected AST summary and rejects negative cases
//! with a diagnostic containing the expected class-substring.
//!
//! See `standard/00-introduction.mdx` § 0.7 for conformance requirements.
//!
//! # cc-* fixtures
//!
//! Fixtures whose names begin with `cc-` exercise the cook_cc module.
//! The *parse-only* gate here validates only the Cookfile AST shape (the
//! `use cook_cc` statement and Lua step syntax).  Runtime execution of
//! cc-* fixtures — actually invoking `cook` against a tempdir, compiling
//! C sources, and asserting outputs — is deferred to a separate runner
//! (path b of the Step-5 design choice in SHI-133 Task 20).
//!
//! When that runner lands it will consume `standard/conformance/_shared/`
//! (populated by `ensure_shared_cook_cc` below) and wire `install_cc_into`
//! into the per-fixture tempdir setup.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use cook_lang::ast::*;
use cook_lang::parse;

// ---------------------------------------------------------------------------
// cook_cc shared installation — used by execute-mode cc-* fixtures
// ---------------------------------------------------------------------------

/// Resolves to `standard/conformance/_shared/cook_cc/share/lua/5.4/cook_cc`.
fn shared_cook_cc_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../standard/conformance/_shared/cook_cc/share/lua/5.4/cook_cc")
}

/// Ensures `standard/conformance/_shared/cook_cc/` is populated exactly once
/// per test run.
///
/// Resolution order:
///   1. `COOK_CC_PATH` env var — copy the `.lua` files from that directory
///      (dev workflow: point at the local cook_cc source tree).
///   2. Otherwise, run `luarocks install cook_cc` against a single server
///      (rocks.usecook.com) to avoid the luarocks dual-server bug.
///
/// This function is a no-op if the cook_cc `init.lua` already exists in the
/// shared tree (idempotent re-runs).
fn ensure_shared_cook_cc() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let shared = shared_cook_cc_dir();
        if shared.join("init.lua").exists() {
            return;
        }
        std::fs::create_dir_all(&shared).expect("mkdir _shared/cook_cc");

        if let Ok(local) = std::env::var("COOK_CC_PATH") {
            let src = PathBuf::from(&local);
            for entry in std::fs::read_dir(&src).expect("read COOK_CC_PATH") {
                let entry = entry.unwrap();
                let name = entry.file_name();
                if name.to_string_lossy().ends_with(".lua") {
                    std::fs::copy(entry.path(), shared.join(&name)).expect("copy lua");
                }
            }
        } else {
            let tree = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../standard/conformance/_shared");
            let status = std::process::Command::new("luarocks")
                .args([
                    "install",
                    "cook_cc",
                    "--tree",
                    tree.to_str().unwrap(),
                    "--server",
                    "https://rocks.usecook.com",
                ])
                .status()
                .expect("luarocks install cook_cc");
            assert!(status.success(), "luarocks install cook_cc failed");
        }
    });
}

/// Symlinks (or copies on non-Unix) the shared cook_cc tree into a fixture's
/// tempdir `cook_modules/` path, so that `use cook_cc` resolves at runtime.
///
/// Called from execute-mode cc-* fixture runners; not called from the
/// parse-only paths in this file.
#[allow(dead_code)]
fn install_cc_into(fixture_tmpdir: &std::path::Path) {
    ensure_shared_cook_cc();

    let target_parent = fixture_tmpdir.join("cook_modules/share/lua/5.4");
    std::fs::create_dir_all(&target_parent).unwrap();
    let shared = shared_cook_cc_dir();
    let dst = target_parent.join("cook_cc");
    if dst.exists() {
        return;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&shared, &dst).unwrap();
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dst).unwrap();
        for entry in std::fs::read_dir(&shared).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
        }
    }
}

fn corpus_root() -> PathBuf {
    if let Ok(override_path) = std::env::var("COOK_CONFORMANCE_CORPUS") {
        return PathBuf::from(override_path)
            .canonicalize()
            .unwrap_or_else(|e| panic!("COOK_CONFORMANCE_CORPUS does not resolve: {}", e));
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../standard/conformance")
        .canonicalize()
        .expect("conformance corpus root missing")
}

fn case_dirs(sub: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let dir = corpus_root().join(sub);
    for entry in fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {}", dir.display(), e))
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn repr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c    => out.push(c),
        }
    }
    out.push('"');
    out
}

fn repr_list(xs: &[String]) -> String {
    let inner: Vec<String> = xs.iter().map(|s| repr(s)).collect();
    format!("[{}]", inner.join(", "))
}

fn format_using(u: &Option<UsingClause>) -> String {
    match u {
        None => "None".to_string(),
        Some(UsingClause::LuaBlock(s))    => format!("LuaBlock({})", repr(s)),
        Some(UsingClause::ShellBlock(xs)) => format!("ShellBlock({})", repr_list(xs)),
    }
}

fn repr_body(body: &Body) -> String {
    match body {
        Body::ShellBlock(lines) if lines.len() == 1 => {
            format!("ShellBlock([{}])", repr(lines[0].trim()))
        }
        Body::ShellBlock(lines) => {
            let inner: Vec<String> = lines.iter().map(|l| repr(l.trim())).collect();
            format!("ShellBlock([{}])", inner.join(", "))
        }
        Body::LuaBlock(code) => format!("LuaBlock({})", repr(code)),
    }
}

fn format_step(step: &Step) -> String {
    match step {
        Step::Shell { command, interactive, .. } => {
            format!("Shell interactive={} command={}", interactive, repr(command))
        }
        Step::Lua { code, .. } => format!("Lua code={}", repr(code)),
        Step::LuaBlock { code, .. } => format!("LuaBlock code={}", repr(code)),
        Step::InlineLua { code, .. } => format!("InlineLua code={}", repr(code)),
        Step::InlineLuaBlock { code, .. } => format!("InlineLuaBlock code={}", repr(code)),
        Step::Cook { step, .. } => {
            format!(
                "Cook outputs={} using={}",
                repr_list(&step.outputs),
                format_using(&step.using_clause),
            )
        }
        Step::Plate { step, .. } => format!("Plate body={}", repr_body(&step.body)),
        Step::Test { step, .. } => {
            let timeout = match step.timeout {
                None    => "None".to_string(),
                Some(n) => format!("Some({})", n),
            };
            format!(
                "Test body={} timeout={} should_fail={}",
                repr_body(&step.body),
                timeout,
                step.should_fail,
            )
        }
        // `Step` is `#[non_exhaustive]`; render unknown future variants with a
        // generic placeholder so the conformance harness keeps building when
        // the AST grows.
        _ => "Step(unknown)".to_string(),
    }
}

fn format_use(u: &UseStatement) -> String {
    format!("UseStatement module_name={} line={}", repr(&u.module_name), u.line)
}

fn format_import(i: &ImportDecl) -> String {
    format!(
        "ImportDecl name={} path={} line={}",
        repr(&i.name),
        repr(&i.path.to_string()),
        i.line,
    )
}

fn format_config(cb: &ConfigBlock) -> String {
    let name = match &cb.name {
        None    => "None".to_string(),
        Some(n) => format!("Some({})", repr(n)),
    };
    format!("ConfigBlock name={} body={} line={}", name, repr(&cb.body), cb.line)
}

fn format_cookfile(c: &Cookfile) -> String {
    let mut out = String::new();
    out.push_str("Cookfile\n");

    let uses: Vec<String> = c.uses.iter().map(format_use).collect();
    out.push_str(&format!("  uses: [{}]\n", uses.join(", ")));

    let imports: Vec<String> = c.imports.iter().map(format_import).collect();
    out.push_str(&format!("  imports: [{}]\n", imports.join(", ")));

    let configs: Vec<String> = c.config_blocks.iter().map(format_config).collect();
    out.push_str(&format!("  config_blocks: [{}]\n", configs.join(", ")));

    out.push_str("  recipes:\n");
    for r in &c.recipes {
        out.push_str(&format!(
            "    Recipe name={} line={}\n",
            repr(&r.name),
            r.line,
        ));
        out.push_str(&format!("      deps: {}\n", repr_list(&r.deps)));
        out.push_str(&format!("      ingredients: {}\n", repr_list(&r.ingredients)));
        out.push_str(&format!("      excludes: {}\n", repr_list(&r.excludes)));
        out.push_str("      steps:\n");
        for s in &r.steps {
            out.push_str(&format!("        {}\n", format_step(s)));
        }
    }

    out.push_str("  chores:\n");
    for ch in &c.chores {
        out.push_str(&format!(
            "    Chore name={} line={}\n",
            repr(&ch.name),
            ch.line,
        ));
        out.push_str(&format!("      deps: {}\n", repr_list(&ch.deps)));
        out.push_str("      steps:\n");
        for s in &ch.steps {
            out.push_str(&format!("        {}\n", format_step(s)));
        }
    }

    out
}

fn normalize(s: &str) -> String {
    let mut lines: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

#[test]
fn positive_conformance_corpus() {
    let mut failures: Vec<String> = Vec::new();

    for case in case_dirs("positive") {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let input_path = case.join("Cookfile");
        let expected_path = case.join("parse.txt");

        let input = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));
        let expected = fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("read {}: {}", expected_path.display(), e));

        match parse(&input) {
            Ok(ast) => {
                let actual = format_cookfile(&ast);
                if normalize(&actual) != normalize(&expected) {
                    failures.push(format!(
                        "case {}: AST shape mismatch.\n--- expected (parse.txt) ---\n{}\n--- actual ---\n{}\n",
                        name,
                        normalize(&expected),
                        normalize(&actual),
                    ));
                }
            }
            Err(e) => {
                failures.push(format!(
                    "case {}: expected parse success, got error: {}\n",
                    name, e
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "positive conformance failures:\n\n{}",
        failures.join("\n")
    );
}

#[test]
fn negative_conformance_corpus() {
    let mut failures: Vec<String> = Vec::new();

    for case in case_dirs("negative") {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let input_path = case.join("Cookfile");
        let expected_path = case.join("error.txt");

        // Fixtures that carry only a `codegen_error.txt` assert a
        // post-parse rejection (e.g. `{lib.ACCESSOR}` without a driver,
        // § 5.4). The parser-only harness skips them; the companion
        // harness in `cook-luagen/tests/conformance.rs` consumes them.
        if !expected_path.exists() && case.join("codegen_error.txt").exists() {
            continue;
        }
        // Fixtures that carry only a `register_error.txt` assert a
        // register-phase rejection (e.g. `cook.add_unit` directory input
        // rejection, § 6.2). The parser-only harness skips them; the
        // companion harness in `cook-register/tests/conformance.rs`
        // consumes them.
        if !expected_path.exists() && case.join("register_error.txt").exists() {
            continue;
        }

        let input = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));
        let expected_substring = fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("read {}: {}", expected_path.display(), e))
            .trim()
            .to_string();

        match parse(&input) {
            Ok(_) => {
                failures.push(format!(
                    "case {}: expected parse error, got success\n",
                    name
                ));
            }
            Err(e) => {
                let msg = format!("{}", e);
                if !msg.contains(&expected_substring) {
                    failures.push(format!(
                        "case {}: error did not contain expected substring\n  expected substring: {:?}\n  actual message:     {:?}\n",
                        name, expected_substring, msg,
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "negative conformance failures:\n\n{}",
        failures.join("\n")
    );
}

#[test]
fn conformance_summary() {
    let root = corpus_root();
    eprintln!(
        "cook-lang claims Cook Standard v{} (corpus: {})",
        cook_lang::COOK_STANDARD_VERSION,
        root.display(),
    );
}
