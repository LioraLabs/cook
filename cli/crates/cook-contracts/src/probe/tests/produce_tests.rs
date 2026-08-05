use crate::probe::lower_produce;

/// The lowering is byte-pinned: these exact bytes used to be spelled once
/// per phase (cook-register's pre-pass and cook-luaotp's worker), and a
/// one-line drift between them would have made a produce body's error
/// report different line numbers depending on which phase evaluated it.
#[test]
fn lowering_bytes_are_pinned() {
    let l = lower_produce("cc:toolchain", "return { cc = \"clang\" }");
    assert_eq!(l.chunk_name, "@probe:cc:toolchain");
    assert_eq!(
        l.source,
        "return (function()\nreturn { cc = \"clang\" }\nend)()"
    );
}

/// The wrapper adds exactly one line above the body, so a Lua error on
/// wrapped line N is body line N-1 — in both phases, by construction.
#[test]
fn wrapper_adds_exactly_one_line_above_the_body() {
    let body = "local a = 1\nerror(\"boom\")";
    let l = lower_produce("k", body);
    let body_start = l.source.find(body).expect("body embedded verbatim");
    let lines_above = l.source[..body_start].matches('\n').count();
    assert_eq!(lines_above, 1);
}

/// The body is embedded verbatim — no trimming, no reindent — because any
/// normalisation would shift line attribution.
#[test]
fn body_is_embedded_verbatim() {
    let body = "  -- indented\n\nreturn 1\n";
    let l = lower_produce("k", body);
    assert!(l.source.contains(body));
}
