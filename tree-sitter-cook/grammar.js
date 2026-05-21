/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// tree-sitter-cook claims conformance with Cook Standard v0.4 + CS-0022.
// The grammar is STALE relative to v0.7 (cs-standard/v0.7); it does not
// implement CS-0023 onward (plate/test block bodies, `//`-anchored sigil
// imports). The `$<IDENT>` placeholder shape from §2.11 IS recognized
// inside string literals and shell text; resolution is the Rust parser's
// concern. See standard/src/content/docs/appendix/A-grammar.mdx for the
// normative grammar; the broader catch-up is tracked by CS-0002.

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
        optional(seq(":", $.dependency_list)),
      ),

    _chore_item: ($) =>
      choice(
        $.inline_lua_line,
        $.inline_lua_block,
        $.lua_line,
        $.lua_block,
        $.interactive_command,
        $.shell_command,
        $.comment,
        $._newline,
      ),

    // ── Recipe body ────────────────────────────────────────────

    _recipe_item: ($) =>
      choice(
        $.ingredients_step,
        $.cook_step,
        $.plate_step,
        $.test_step,
        $.inline_lua_line,
        $.inline_lua_block,
        $.lua_line,
        $.lua_block,
        $.interactive_command,
        $.shell_command,
        $.comment,
        $._newline,
      ),

    // App. A.4 + CS-0078 multi-line patterns:
    //   ingredients_step ::= "ingredients" ingredient (CONT? ingredient)* NEWLINE
    //   ingredient       ::= STRING | "!" STRING
    // CONT is an external token (_step_continuation_newline) emitted only
    // when the next line begins with `"` or `!"`; otherwise the declaration
    // terminates and the next line dispatches per App. A.4's priority order.
    ingredients_step: ($) =>
      seq(
        "ingredients",
        choice($.string, $.ingredient_exclude),
        repeat(seq(
          optional($._step_continuation_newline),
          choice($.string, $.ingredient_exclude),
        )),
        $._newline,
      ),

    ingredient_exclude: ($) => seq("!", $.string),

    // App. A.4 multi-output rule: when two or more outputs are given,
    // the using_clause MUST be a block (`>{...}` or `{...}`); a bare
    // string is rejected. The single-output form keeps all four shapes
    // (declaration-only, string, lua block, shell block).
    // CS-0078: subsequent output STRINGs MAY appear on continuation lines
    // beginning with `"`; the `_step_continuation_newline` external token
    // absorbs the intervening newline + whitespace.
    cook_step: ($) =>
      choice(
        seq(
          "cook",
          field("outputs", $.string),
          optional($.using_clause),
          $._newline,
        ),
        seq(
          "cook",
          field("outputs", $.string),
          repeat1(seq(
            optional($._step_continuation_newline),
            field("outputs", $.string),
          )),
          $.block_using_clause,
          $._newline,
        ),
      ),

    using_clause: ($) =>
      seq(
        "using",
        choice(
          field("lua", $.using_lua_block),
          field("shell", $.shell_block),
        ),
      ),

    block_using_clause: ($) =>
      seq(
        "using",
        choice(
          field("lua", $.using_lua_block),
          field("shell", $.shell_block),
        ),
      ),

    // App. A.4 `using_lua_block`: the `>{ … }` execute-phase Lua block in a
    // using clause, with §6.4 input/output bindings. Distinct in name from
    // the recipe-body step kind `inline_lua_block` (`>>{ … }`, register-phase).
    using_lua_block: ($) =>
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

    plate_step: ($) =>
      seq(
        "plate",
        field("body", choice($.shell_block, $.using_lua_block)),
        $._newline,
      ),

    test_step: ($) =>
      seq(
        "test",
        field("body", choice($.shell_block, $.using_lua_block)),
        field("as_name", optional(seq("as", $.string))),
        optional(seq("timeout", field("timeout", $.number))),
        optional(field("should_fail", "should_fail")),
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

    // §{recipes.lua-steps} register-phase forms: `>>` line and `>>{ ... }`
    // block. Both desugar at the lexer / scanner level — `>>` and `>>{`
    // tokens, then the same brace-balanced LUA_BLOCK_CONTENT collection
    // for the block form.
    inline_lua_line: ($) =>
      seq(
        ">>",
        alias(token.immediate(/[^{\n][^\n]*/), $.lua_code),
        $._newline,
      ),

    inline_lua_block: ($) =>
      seq(">>{", alias($._lua_block_content, $.lua_code), "}", $._newline),

    // App. A.1 + A.4 top-level `module_call` (CS-0072). A column-0
    // `LUA_IDENT . IDENT_START …` statement, brace-balanced across
    // newlines per §{lexical.brace-blocks.lua-spans}. The full text
    // is collected by the external scanner so multi-line table-arg
    // forms (`cook_cc.bin("game", {\n  …\n})`) parse as a single
    // statement. Resolution of Lua-expression-hood is the runtime's
    // concern, not the grammar's. Per CS-0072, recipe-body bare
    // `LUA_IDENT.IDENT_START…` is shell, not module_call — the
    // recipe-body cascade in `_recipe_item` no longer carries a
    // module_call arm.
    top_level_module_call: ($) =>
      seq(
        alias($._top_level_module_call_text, $.module_call_text),
        $._newline,
      ),

    interactive_command: ($) =>
      seq(
        "@",
        repeat1(
          choice(
            alias(token.immediate(/[^\n$]+/), $.shell_content),
            alias(token.immediate("$"), $.shell_content),
            $.placeholder,
          ),
        ),
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
    placeholder: ($) =>
      seq(
        "$<",
        alias(
          token.immediate(/[A-Za-z_][A-Za-z0-9_.]*/),
          $.placeholder_ident,
        ),
        token.immediate(">"),
      ),

    _string_placeholder: ($) =>
      seq(
        token.immediate("$<"),
        alias(
          token.immediate(/[A-Za-z_][A-Za-z0-9_.]*/),
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
          token.immediate(/[A-Za-z_][A-Za-z0-9_.]*/),
          $.placeholder_ident,
        ),
        token.immediate(">"),
      ),

    path: ($) => /[^\s\n]+/,

    number: ($) => /[0-9]+/,
  },
});
