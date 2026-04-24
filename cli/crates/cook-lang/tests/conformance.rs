//! Conformance corpus harness.
//!
//! Walks `standard/conformance/` and asserts that `cook-lang` parses
//! positive cases into the expected AST summary and rejects negative cases
//! with a diagnostic containing the expected class-substring.
//!
//! See `standard/00-introduction.mdx` § 0.7 for conformance requirements.

use std::fs;
use std::path::PathBuf;

use cook_lang::ast::*;
use cook_lang::parse;

fn corpus_root() -> PathBuf {
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
        Some(UsingClause::Shell(s))       => format!("Shell({})", repr(s)),
        Some(UsingClause::LuaBlock(s))    => format!("LuaBlock({})", repr(s)),
        Some(UsingClause::ShellBlock(xs)) => format!("ShellBlock({})", repr_list(xs)),
    }
}

fn format_step(step: &Step) -> String {
    match step {
        Step::Shell { command, interactive, .. } => {
            format!("Shell interactive={} command={}", interactive, repr(command))
        }
        Step::Lua { code, .. } => format!("Lua code={}", repr(code)),
        Step::LuaBlock { code, .. } => format!("LuaBlock code={}", repr(code)),
        Step::Cook { step, .. } => {
            format!(
                "Cook outputs={} using={}",
                repr_list(&step.outputs),
                format_using(&step.using_clause),
            )
        }
        Step::Plate { step, .. } => format!("Plate command={}", repr(&step.command)),
        Step::Test { step, .. } => {
            let timeout = match step.timeout {
                None    => "None".to_string(),
                Some(n) => format!("Some({})", n),
            };
            format!(
                "Test command={} timeout={} should_fail={}",
                repr(&step.command),
                timeout,
                step.should_fail,
            )
        }
    }
}

fn format_use(u: &UseStatement) -> String {
    format!("UseStatement module_name={} line={}", repr(&u.module_name), u.line)
}

fn format_import(i: &ImportDecl) -> String {
    format!(
        "ImportDecl name={} path={} line={}",
        repr(&i.name),
        repr(&i.path),
        i.line,
    )
}

fn format_var(v: &(String, String)) -> String {
    format!("({}, {})", repr(&v.0), repr(&v.1))
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

    let vars: Vec<String> = c.vars.iter().map(format_var).collect();
    out.push_str(&format!("  vars: [{}]\n", vars.join(", ")));

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
