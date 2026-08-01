§{lexical.source-representation}, COOK-398. Pins that a control character
appearing inside a shell body survives lowering, rather than being rejected by
the Lua the implementation happens to generate.

§3.1 requires a conforming implementation to accept any valid UTF-8 input. A
carriage return is valid UTF-8. §3.1 also gives `\r` a second, narrower job: a
CR immediately before a line terminator is trailing whitespace and is trimmed
before classification. A CR in the *middle* of a line is neither a terminator
nor trailing whitespace, so it is ordinary body text and reaches the command
string intact, exactly as §{lexical.brace-blocks}'s per-segment trim leaves it.

The reference implementation did not accept it. `cook-luagen`'s
`escape_lua_string` escaped `\`, `"` and newline, and nothing else. Lua forbids
a raw carriage return inside a short string literal, so the generated program
failed to load and the build died before doing any work:

    cook: syntax error: Cookfile:7: unfinished string near '"set -e
    echo "a

The line number in that diagnostic is the generated Lua's, not the Cookfile's,
which is why the failure was hard to recognise as an escaping bug. The most
likely way to meet it in practice is a Cookfile saved with CRLF line endings by
an editor that also left a stray CR inside a quoted string.

Fixed by moving the rule to `cook_contracts::lua_string::escape_double_quoted`,
which `cook-register`'s chore-param prelude already needed to agree with; the
two had drifted, and register's version had its own defect (it emitted `\0`
where Lua's decimal escape consumes up to three digits, so `\0` followed by a
digit is a different character).

**Why this fixture carries `register_ok.txt` and not just `parse.txt`.** The
Cookfile always parsed. The failure was one stage later, in codegen and the
load of the generated chunk, so a parse-only case would have passed against the
broken implementation and pinned nothing. Verified to bite by reverting the
escaper and re-running: the register harness fails with the syntax error above.

**Reading this fixture.** `Cookfile` and `parse.txt` both contain a literal
`U+000D` byte on the `echo` line, which will not be visible in most editors. Use
`od -c` to see it. Do not "fix" the file by deleting the stray CR: it is the
subject of the test.
