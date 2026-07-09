//! One-shot helper: parse a Cookfile and emit its conformance parse.txt
//! shape on stdout. Usage: `cargo run -p cook-lang --example dump_parse -- <path>`

use std::env;
use std::fs;

use cook_lang::ast::*;
use cook_lang::parse;

fn repr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn repr_list(xs: &[String]) -> String {
    let inner: Vec<String> = xs.iter().map(|s| repr(s)).collect();
    format!("[{}]", inner.join(", "))
}

fn format_output_patterns(xs: &[OutputPattern]) -> String {
    let inner: Vec<String> = xs
        .iter()
        .map(|p| match p {
            OutputPattern::Quoted(s) => repr(s),
            OutputPattern::LuaExpr(s) => format!("LuaExpr({})", repr(s)),
        })
        .collect();
    format!("[{}]", inner.join(", "))
}

fn format_using(u: &Option<Body>) -> String {
    match u {
        None => "None".to_string(),
        Some(Body::LuaBlock(s)) => format!("LuaBlock({})", repr(s)),
        Some(Body::ShellBlock(xs)) => format!("ShellBlock({})", repr_list(xs)),
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
        Step::Cook { step, .. } => {
            format!(
                "Cook outputs={} using={}",
                format_output_patterns(&step.outputs),
                format_using(&step.body),
            )
        }
        Step::Plate { step, .. } => format!("Plate body={}", repr_body(&step.body)),
        Step::Test { step, .. } => {
            let timeout = match step.timeout {
                None => "None".to_string(),
                Some(n) => format!("Some({})", n),
            };
            format!(
                "Test body={} timeout={} should_fail={}",
                repr_body(&step.body),
                timeout,
                step.should_fail,
            )
        }
        _ => "<unknown Step variant>".to_string(),
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
        None => "None".to_string(),
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

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: dump_parse <Cookfile-path>");
        std::process::exit(2);
    }
    let src = fs::read_to_string(&args[1]).expect("read input");
    match parse(&src) {
        Ok(ast) => {
            print!("{}", format_cookfile(&ast));
        }
        Err(e) => {
            eprintln!("parse error: {}", e);
            std::process::exit(1);
        }
    }
}
