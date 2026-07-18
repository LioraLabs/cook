/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// Conforms to Cook Standard v0.14. See standard/src/content/docs/
// appendix/A-grammar.mdx for the normative grammar.

module.exports = grammar({
  name: "cook",

  extras: ($) => [/[ \t\r]/],

  externals: ($) => [
    $._lua_block_content,
    $._shell_content,
    $._config_block_content,
    $._shell_block_content,
    $._register_block_content,
    $._top_level_module_call_text,
    $._step_continuation_newline,
  ],

  word: ($) => $._bare_identifier,

  rules: {
    source_file: ($) => repeat($._toplevel_item),

    _toplevel_item: ($) =>
      choice(
        $.recipe,
        $.chore,
        $.probe,
        $.config_block,
        $.register_block,
        $.top_level_module_call,
        $.use_declaration,
        $.import_declaration,
        $.comment,
        $._newline,
      ),

    comment: ($) => token(seq("#", /[^\n]*/)),
    _newline: ($) => /\n/,

    // ── Top-level declarations ─────────────────────────────────

    use_declaration: ($) =>
      seq("use", field("module", $._lua_ident_name), $._newline),

    import_declaration: ($) =>
      seq(
        "import",
        field("name", alias($._bare_identifier, $.import_name)),
        field("path", $.path),
        $._newline,
      ),

    config_block: ($) =>
      seq(
        "config",
        optional(field("name", $._name)),
        $._newline,
        alias($._config_block_content, $.lua_code),
      ),

    // App. A.1 + CS-0072. A top-level `register` block carries Lua source
    // that runs at register-phase before any recipe declarations. The body
    // ranges from the line after the header to the next column-0 line
    // classified as `recipe`, `chore`, `config`, `use`, `import`, `register`,
    // or (per issue COOK-51) a top-level module_call. The body MAY be empty
    // (`register-block-empty` fixture is literally just the word `register`),
    // so the lua_code content is optional. The terminating NEWLINE is also
    // optional because EOF can follow the keyword directly.
    register_block: ($) =>
      seq(
        "register",
        $._newline,
        optional(alias($._register_block_content, $.lua_code)),
      ),

    // ── Recipes ────────────────────────────────────────────────

    recipe: ($) =>
      prec.right(
        seq(
          $._recipe_header,
          $._newline,
          repeat($._recipe_item),
        ),
      ),

    _recipe_header: ($) => $.explicit_recipe_header,

    explicit_recipe_header: ($) =>
      prec(
        1,
        seq(
          "recipe",
          field("name", $._decl_name),
          optional(seq(":", $.dependency_list)),
        ),
      ),

    dependency_list: ($) => repeat1($._name),

    // ── Chores ─────────────────────────────────────────────────

    chore: ($) =>
      prec.right(
        seq(
          $.chore_header,
          $._newline,
          repeat($._chore_item),
        ),
      ),

    chore_header: ($) =>
      seq(
        "chore",
        field("name", $._decl_name),
        field("params", optional($.chore_param_list)),
        optional(seq(":", $.dependency_list)),
      ),

    // COOK-36 / Standard §7.1.1 chore parameters. The grammar accepts
    // any order of param variants; spec ordering rules (required →
    // defaulted → at-most-one variadic), reserved-name ban, dot-ban,
    // and duplicate-name detection are semantic checks enforced by the
    // Rust parser, not tree-sitter.
    chore_param_list: ($) => repeat1($.chore_param),

    chore_param: ($) =>
      choice(
        $.required_param,
        $.defaulted_param,
        $.variadic_param,
      ),

    required_param: ($) => field("name", $._chore_param_name),

    defaulted_param: ($) =>
      seq(
        field("name", $._chore_param_name),
        "=",
        field("default", choice($.string, $.lua_expr_default)),
      ),

    variadic_param: ($) =>
      seq(
        field("sigil", choice("+", "*")),
        field("name", $._chore_param_name),
      ),

    // Param-name shape: bare ASCII Lua-identifier. Tighter than
    // `_decl_bare` (no `-`, no `.`); a stricter superset of the
    // §7.1.1 grammar would be enforced by the Rust parser anyway.
    // The rule is hidden (`_`-prefix) so the tree carries
    // `name: (identifier)` rather than a redundant wrapper node.
    _chore_param_name: ($) =>
      alias(token(/[A-Za-z_][A-Za-z0-9_]*/), $.identifier),

    // `=( LUA_EXPR )` default. Tree-sitter doesn't parse Lua syntax;
    // it just scans a balanced-paren region with string awareness so
    // that nested parens (`cook.git.head_tag()`) and parens inside
    // strings (`"(boom)"`) don't break the scan. Multi-line Lua-expr
    // defaults are out of scope (chore_header is a single line).
    lua_expr_default: ($) =>
      seq(
        "(",
        repeat($._lua_expr_chunk),
        ")",
      ),

    _lua_expr_chunk: ($) =>
      choice(
        $.lua_expr_default, // nested parens
        token(/"(?:[^"\\\n]|\\[^\n])*"/),
        token(/'(?:[^'\\\n]|\\[^\n])*'/),
        token(prec(-1, /[^()"'\n]+/)),
      ),

    _chore_item: ($) =>
      choice(
        $.lua_line,
        $.lua_block,
        $.shell_command,
        $.comment,
        $._newline,
      ),

    // ── Probes (COOK-67/68/69, §22, App. A.3.2; CS-0092 / v0.14) ──
    //
    //   probe_decl   ::= "probe" probe_name (":" probe_dep_list)? NEWLINE
    //                    INDENT probe_body DEDENT
    //   probe_body ::= "files" glob_list NEWLINE
    //                | ingredients_step? producer NEWLINE
    //   producer   ::= ("json" | "lines")? shell_block
    //                | ("tools" | "envs") name_list
    //                | exec_lua_block
    //
    // The body region (App. A.3.2 "Column-zero constraint" + the
    // implicit-termination rule) is handled the same way as recipes:
    // the scanner stops a preceding recipe/chore/config/register body at
    // a column-0 `probe NAME` line (`is_step_keyword`-sibling check in
    // scan_shell_content, plus `is_toplevel_keyword`), and the probe body
    // itself contains no `shell_command`, so it terminates naturally at
    // the next column-0 top-level item once `producer` closes.
    //
    // CS-0148: a `files` producer MUST NOT combine with a preceding
    // `ingredients_step` (§22.5.2, A-grammar A.3.2). This is encoded
    // syntactically, not just semantically: the `files` form is a
    // separate first alternative of the top-level choice below, so an
    // `ingredients` line followed by `files { … }` cannot reduce through
    // that branch (no `ingredients_step` slot precedes it) and instead
    // falls into the `producer` branch, where `files` is not a valid
    // producer keyword — producing an ERROR node.
    probe: ($) =>
      seq(
        $.probe_header,
        $._newline,
        repeat(choice($._newline, $.comment)),
        choice(
          alias($.files_producer, $.producer),
          seq(
            optional(seq(
              $.ingredients_step,
              repeat(choice($._newline, $.comment)),
            )),
            $.producer,
          ),
        ),
      ),

    // `files` is a contextual keyword recognised only in this probe-body
    // position (mirrors `tools`/`envs`/`json`/`lines` in `producer`
    // below). Aliased to `$.producer` so the tree shape stays uniform
    // with the other producer kinds (a single `producer` node per probe).
    files_producer: ($) =>
      seq(
        "files",
        $.glob_list,
        $._newline,
      ),

    probe_header: ($) =>
      seq(
        "probe",
        field("name", $._probe_name),
        optional(seq($._probe_dep_colon, $.probe_dep_list)),
      ),

    // probe_name / probe_ref ::= BARE_PROBE_KEY | STRING, where
    // BARE_PROBE_KEY ::= PROBE_SEG (":" PROBE_SEG)? — at most one module-prefix
    // colon. The single-token regex enforces the at-most-one-colon shape
    // by maximal munch: `cc:zlib` lexes as one name, while the third
    // contiguous `:IDENT` of `a:b:c` is left dangling and — since the
    // dep-list colon below requires trailing whitespace — produces an
    // ERROR (App. A.3.2 triple-colon rejection, made syntactic here
    // rather than SEMANTIC_ONLY). See the dep-colon note below.
    _probe_name: ($) =>
      choice(alias($._bare_probe_key, $.identifier), $.string),

    _probe_ref: ($) =>
      choice(alias($._bare_probe_key, $.identifier), $.string),

    _bare_probe_key: ($) =>
      token(prec(-1, /[A-Za-z_][A-Za-z0-9_.-]*(:[A-Za-z_][A-Za-z0-9_.-]*)?/)),

    // Module-prefix-colon disambiguation (App. A.3.2, normative). The
    // dep-list-introducing `:` is distinguished from the module-prefix
    // `:` purely positionally: the dep-list colon is followed by
    // whitespace or end-of-line, the module-prefix colon by an ident
    // char. tree-sitter's token regexes can't express lookahead, so the
    // dep-colon token consumes one trailing whitespace char; maximal
    // munch then prefers it over `_bare_probe_key`'s internal colon only
    // when a space follows (`p: a` → deps), while `cc:zlib` (no space)
    // stays a single name token. A third `:IDENT` with no space (`a:b:c`)
    // matches neither this token nor `_newline`, so it ERRORs.
    _probe_dep_colon: ($) => token(seq(":", /[ \t]/)),

    probe_dep_list: ($) => repeat1($._probe_ref),

    // Producer keywords are contextual because these literals occur only
    // after a probe header. JSON/lines decorate shell output; tools/envs
    // accept a non-empty, one-line list of narrow IDENT names.
    producer: ($) =>
      seq(
        choice(
          seq(optional(choice("json", "lines")), field("body", $.shell_block)),
          seq(choice("tools", "envs"), $.name_list),
          field("body", $.exec_lua_block),
        ),
        $._newline,
      ),

    name_list: ($) =>
      seq(
        "{",
        alias($._lua_ident, $.identifier),
        repeat(seq(optional(","), alias($._lua_ident, $.identifier))),
        "}",
      ),

    // A.3.2 `glob_list` — same brace shape as `name_list` (comma/whitespace
    // separated, single physical line), holding quoted glob patterns with
    // an optional `!` exclude prefix instead of bare IDENTs. Mirrors
    // `name_list`'s convention of requiring the first entry syntactically
    // (an empty `{}` MISSES the first `glob_pattern`, matching how an
    // empty `tools {}`/`envs {}` is rejected above): the A-grammar prose
    // (§22.5.2) requires a conforming implementation to reject an empty
    // `glob_list`.
    glob_list: ($) =>
      seq(
        "{",
        $.glob_pattern,
        repeat(seq(optional(","), $.glob_pattern)),
        "}",
      ),

    glob_pattern: ($) =>
      choice(
        $.string,
        seq("!", $.string),
      ),

    // ── Recipe body ────────────────────────────────────────────

    _recipe_item: ($) =>
      choice(
        seq(
          $._recipe_indent,
          choice(
            $.ingredients_step,
            $.seal_step,
            $.cook_step,
            $.test_step,
            $.module_call,
          ),
        ),
        $.comment,
        $._newline,
      ),

    // App. A.4 + CS-0078 multi-line patterns:
    //   ingredients_step ::= "ingredients" (ingredient+ | probe_ref) NEWLINE
    //   ingredient       ::= STRING | "!" STRING
    // CONT is an external token (_step_continuation_newline) emitted only
    // when the next line begins with `"` or `!"`; otherwise the declaration
    // terminates and the next line dispatches per App. A.4's priority order.
    // The quote-vs-bare discriminator makes filesystem items mutually
    // exclusive with a probe source. A source may add one narrow field
    // selector after the probe key; thus `ns:cards:items` is a two-segment
    // key plus selector without widening probe keys elsewhere.
    ingredients_step: ($) =>
      choice(
        seq(
          "ingredients",
          choice($.string, $.ingredient_exclude),
          repeat(seq(
            optional($._step_continuation_newline),
            choice($.string, $.ingredient_exclude),
          )),
          $._newline,
        ),
        seq(
          "ingredients",
          field("probe", alias($._bare_probe_key, $.identifier)),
          optional(seq(":", field("field", alias($._lua_ident, $.identifier)))),
          $._newline,
        ),
      ),

    ingredient_exclude: ($) => seq("!", $.string),

    // Appendix A recipe-level determinant baseline. `unseal` is trailing-only
    // and is therefore admitted below only as a cook modifier.
    seal_step: ($) =>
      seq(
        "seal",
        repeat1($._disposition_ref),
        $._newline,
      ),

    // App. A.4 multi-output rule: when two or more outputs are given,
    // the body MUST be a block (`>{...}` or `{...}`); a bare string
    // simply joins the output-pattern list. CS-0099: the body opener
    // follows the output pattern(s) directly (no `using` introducer).
    // CS-0078: subsequent output STRINGs MAY appear on continuation lines
    // beginning with `"`; the `_step_continuation_newline` external token
    // absorbs the intervening newline + whitespace.
    cook_step: ($) =>
      seq(
        "cook",
        choice(
          field("outputs", $.lua_expr_output),
          seq(
            field("outputs", $.string),
            repeat(seq(
              optional($._step_continuation_newline),
              field("outputs", $.string),
            )),
          ),
        ),
        field("body", choice($.shell_block, $.exec_lua_block)),
        optional($.cook_mods),
        $._newline,
      ),

    cook_mods: ($) =>
      choice(
        seq(
          repeat1(choice($.seal_group, $.unseal_group)),
          optional($.share_mod),
        ),
        $.share_mod,
      ),

    seal_group: ($) =>
      seq("seal", repeat1($._disposition_ref)),

    unseal_group: ($) =>
      seq("unseal", repeat1($._disposition_ref)),

    _disposition_ref: ($) =>
      choice(
        alias(token(prec(1, /[A-Za-z_][A-Za-z0-9_.-]*:[A-Za-z_][A-Za-z0-9_.-]*/)), $.identifier),
        alias($._bare_probe_key, $.identifier),
      ),

    share_mod: ($) => choice("local", "pinned", "nondet"),

    // §8.4.2 (CS-0089): `cook (LUA_EXPR)` — a parenthesised Lua expression
    // in the output slot, evaluated once per ingredient at register time.
    // Balanced-paren interior with single-level quoted-string opacity
    // (mirrors the §7.1.1 chore-param scanner; Lua long-brackets are NOT
    // handled, per the documented v1 limitation in §8.4.2).
    // Reuses `_lua_expr_chunk` from the §7.1.1 chore-param scanner above
    // (same balanced-paren + quoted-string opacity contract).
    lua_expr_output: ($) => seq("(", repeat($._lua_expr_chunk), ")"),

    // App. A.4 `exec_lua_block`: the `>{ … }` execute-phase Lua block in a
    // cook/test body, with §6.4 input/output bindings.
    exec_lua_block: ($) =>
      seq(">{", alias($._lua_block_content, $.lua_code), "}"),

    // Body chunks: scanner-emitted SHELL_BLOCK_CONTENT segments interleaved
    // with `$<IDENT>` placeholders (§2.11). The scanner stops at each
    // valid placeholder boundary so the grammar can lex it as a token.
    shell_block: ($) =>
      seq(
        "{",
        repeat(
          choice(
            alias($._shell_block_content, $.shell_content),
            $.placeholder,
          ),
        ),
        "}",
      ),

    test_step: ($) =>
      seq(
        "test",
        field("body", choice($.shell_block, $.exec_lua_block)),
        $._newline,
      ),

    lua_line: ($) =>
      seq(
        ">",
        alias(token.immediate(/[^{\n][^\n]*/), $.lua_code),
        $._newline,
      ),

    lua_block: ($) =>
      seq(">{", alias($._lua_block_content, $.lua_code), "}", $._newline),

    // App. A.1 + A.4 top-level `module_call` (CS-0072). A column-0
    // `LUA_IDENT . IDENT_START …` statement, brace-balanced across
    // newlines per §{lexical.brace-blocks.lua-spans}. The full text
    // is collected by the external scanner so multi-line table-arg
    // forms (`cook_cc.bin("game", {\n  …\n})`) parse as a single
    // statement. Resolution of Lua-expression-hood is the runtime's
    // concern, not the grammar's. Recipe bodies reuse this exact token.
    module_call: ($) =>
      seq(
        alias($._top_level_module_call_text, $.module_call_text),
        $._newline,
      ),

    top_level_module_call: ($) =>
      seq(
        alias($._top_level_module_call_text, $.module_call_text),
        $._newline,
      ),

    shell_command: ($) =>
      seq(
        repeat1(
          choice(
            alias($._shell_content, $.shell_content),
            $.placeholder,
          ),
        ),
        $._newline,
      ),

    // ── Primitives ─────────────────────────────────────────────

    _name: ($) => choice(alias($._bare_identifier, $.identifier), $.string),

    _bare_identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_.\-]*/,

    // Recipe-body module calls are indented; retaining the indentation as a
    // hidden immediate token keeps a following column-zero module call in
    // the top-level dispatch without introducing another external token.
    _recipe_indent: ($) => token.immediate(/[ \t]+/),

    // CS-0035 declaration-site no-dots. `recipe_header` and `chore_header`
    // use this stricter name shape: dots are rejected. Hyphens remain
    // legal (e.g. `recipe my-task`). The quoted form is also dot-free.
    _decl_name: ($) =>
      choice(
        alias($._decl_bare, $.identifier),
        alias($._decl_string, $.string),
      ),
    _decl_bare: ($) => /[A-Za-z_][A-Za-z0-9_\-]*/,
    _decl_string: ($) => /"[^"\.\n]*"/,

    // CS-0035 use_name LUA_IDENT constraint. `use_declaration`'s name is
    // bound at load time as a Lua local under the same spelling, so it
    // MUST be a strict Lua identifier: no dots, no hyphens, no spaces.
    _lua_ident_name: ($) =>
      choice(
        alias($._lua_ident, $.identifier),
        alias($._lua_ident_string, $.string),
      ),
    _lua_ident: ($) => /[A-Za-z_][A-Za-z0-9_]*/,
    _lua_ident_string: ($) => /"[A-Za-z_][A-Za-z0-9_]*"/,

    // §2.11 placeholder. The seq is structured (rather than `token(...)`)
    // so the `$<`/`>` punctuation and the inner identifier can each be
    // captured separately for highlighting. Two surfaces share the byte
    // shape:
    //   • `placeholder` — used in shell text where an external scanner
    //     has stopped at the `$<` boundary; the leading `$<` is matched
    //     non-immediately because the scanner consumed any leading WS.
    //   • `_string_placeholder` — used inside string literals where every
    //     token MUST be immediate to keep extras out of the string body.
    //
    // Current closed placeholder shapes: file references, recipe-member
    // `[in]`, probe-value refs (colon namespace with optional field/index),
    // and dot-separated builtin/accessor/env/recipe identifiers.
    placeholder: ($) =>
      seq(
        "$<",
        alias(
          token.immediate(/file:[A-Za-z0-9_.*\/-]+|[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*\[in\]|[A-Za-z_][A-Za-z0-9_.-]*:[A-Za-z_][A-Za-z0-9_.-]*(?:\.[A-Za-z_][A-Za-z0-9_]*)?(?:\[[0-9]+\])?|[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*/),
          $.placeholder_ident,
        ),
        token.immediate(">"),
      ),

    _string_placeholder: ($) =>
      seq(
        token.immediate("$<"),
        alias(
          token.immediate(/file:[A-Za-z0-9_.*\/-]+|[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*\[in\]|[A-Za-z_][A-Za-z0-9_.-]*:[A-Za-z_][A-Za-z0-9_.-]*(?:\.[A-Za-z_][A-Za-z0-9_]*)?(?:\[[0-9]+\])?|[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*/),
          $.placeholder_ident,
        ),
        token.immediate(">"),
      ),

    // String literals expose their inner placeholders. The fallback
    // `_string_chunk` keeps a bare `$` as text so a real placeholder
    // (which requires `$<`) is the only thing that consumes the `$<`
    // pair. NOTE: §2.11 strict-bail says a malformed `$<bad spaces>`
    // is literal text; with this structured rule the seq commits to
    // `$<` and errors at the missing `>`. The Rust parser remains the
    // source of truth for that edge case.
    // CS-0061: STRING admits both double- and single-quoted forms.
    string: ($) =>
      choice(
        seq(
          '"',
          repeat(
            choice(
              alias($._string_placeholder, $.placeholder),
              $._dq_string_chunk,
            ),
          ),
          token.immediate('"'),
        ),
        seq(
          "'",
          repeat(
            choice(
              alias($._sq_string_placeholder, $.placeholder),
              $._sq_string_chunk,
            ),
          ),
          token.immediate("'"),
        ),
      ),

    _dq_string_chunk: ($) =>
      choice(
        token.immediate(/[^"$]+/),
        token.immediate("$"),
      ),

    _sq_string_chunk: ($) =>
      choice(
        token.immediate(/[^'$]+/),
        token.immediate("$"),
      ),

    _sq_string_placeholder: ($) =>
      seq(
        token.immediate("$<"),
        alias(
          token.immediate(/file:[A-Za-z0-9_.*\/-]+|[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*\[in\]|[A-Za-z_][A-Za-z0-9_.-]*:[A-Za-z_][A-Za-z0-9_.-]*(?:\.[A-Za-z_][A-Za-z0-9_]*)?(?:\[[0-9]+\])?|[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*/),
          $.placeholder_ident,
        ),
        token.immediate(">"),
      ),

    path: ($) => /[^\s\n]+/,

  },
});
