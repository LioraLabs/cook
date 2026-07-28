use super::*;

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_block_is_one_shell_text_under_set_e() {
    assert_eq!(
        compose(&lines(&["mkdir -p build", "cc -o build/x x.c"])),
        "set -e\nmkdir -p build\ncc -o build/x x.c"
    );
}

#[test]
fn a_single_line_block_still_carries_the_prefix() {
    // The inline form `cook "out" { echo hi }` is the same rule with one line;
    // it must not shed `set -e` just because there is nothing after it to abort.
    assert_eq!(compose(&lines(&["echo hi"])), "set -e\necho hi");
}

#[test]
fn an_empty_block_composes_to_a_well_formed_no_op() {
    assert_eq!(compose(&[]), "set -e");
}

#[test]
fn the_prefix_costs_no_source_line() {
    // §8.3.1: line k of the block must be line k of the author's body, so a
    // diagnostic citing a line number cites theirs. `set -e` shares line 1 with
    // nothing, and the first authored line lands on line 2 of the composed text
    // — which is the offset every line-number consumer is entitled to assume is
    // constant, whatever the block's length.
    let composed = compose(&lines(&["first", "second", "third"]));
    let out: Vec<&str> = composed.lines().collect();
    assert_eq!(out[0], "set -e");
    assert_eq!(out[1], "first");
    assert_eq!(out[2], "second");
    assert_eq!(out[3], "third");
}

#[test]
fn lines_are_joined_in_source_order_and_verbatim() {
    // No trimming, no reordering, no blank-line collapsing: normalisation is
    // the lexer's job (§{lexical.brace-blocks}) and has already happened by the
    // time a block reaches here. Doing it again would mean two answers to what
    // a body's text is.
    let authored = lines(&["  indented", "trailing   ", "has  internal   spaces"]);
    assert_eq!(
        compose(&authored),
        "set -e\n  indented\ntrailing   \nhas  internal   spaces"
    );
}

#[test]
fn shell_metacharacters_pass_through_untouched() {
    // The composed text is shell source, and the shell parses it. Escaping here
    // would corrupt every body that uses a pipe, a redirect or a heredoc.
    let authored = lines(&["cat a | grep x > b", "echo 'single' \"double\" $VAR"]);
    assert_eq!(
        compose(&authored),
        "set -e\ncat a | grep x > b\necho 'single' \"double\" $VAR"
    );
}
