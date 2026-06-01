#include "tree_sitter/parser.h"

#include <wctype.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

enum TokenType {
  LUA_BLOCK_CONTENT,
  SHELL_CONTENT,
  CONFIG_BLOCK_CONTENT,
  SHELL_BLOCK_CONTENT,
  REGISTER_BLOCK_CONTENT,
  TOP_LEVEL_MODULE_CALL_TEXT,
  STEP_CONTINUATION_NEWLINE,
};

// Persistent state for the SHELL_BLOCK_CONTENT scanner. A `$<IDENT>`
// placeholder inside a shell-quoted string ("...$<X>...") splits the
// scan across multiple calls; without persisting the in-string state,
// the resumed scan would mistreat the closing `"` as opening a new
// string and the trailing `}` would land in the wrong context.
//
// CS-0035 (COOK-54): POSIX heredoc opaque-span tracking. When a `<<TAG`
// (or `<<-TAG`, `<<'TAG'`, `<<"TAG"`) opener is seen, we record the
// tag in `heredoc_tag[]` and set `in_heredoc=1`. While in_heredoc is
// non-zero, `{` / `}` / quotes are inert. The heredoc terminates on a
// line whose first non-whitespace (or first column-0 char for `<<-=0`)
// is the recorded tag followed by a newline or EOF. Tags exceeding
// HEREDOC_TAG_CAP are an exotic edge case and are NOT tracked (the
// scanner falls back to the pre-CS-0035 behaviour for those).
#define HEREDOC_TAG_CAP 31
typedef struct {
  uint8_t depth;
  uint8_t in_string;          // 0 = outside, 1 = double, 2 = single
  uint8_t in_heredoc;         // 0 = no, 1 = inside heredoc body
  uint8_t heredoc_dash;       // 1 if opener was `<<-` (strip leading tabs)
  uint8_t heredoc_tag_len;
  char    heredoc_tag[HEREDOC_TAG_CAP];
} ShellBlockState;

// §2.11 placeholder lookahead. Returns true if the bytes at the current
// lookahead position form a complete `$<IDENT>` placeholder, where
// IDENT = ALPHA (ALPHA | DIGIT | "_" | ".")*. Advances the lexer cursor
// past the bytes inspected (caller decides whether to include them in
// the surrounding token by calling mark_end before/after, exploiting
// tree-sitter's mark_end semantics: bytes between the last mark_end and
// the cursor are pure lookahead and the next token starts at mark_end).
//
// Strict-bail (§2.11): we never search past the first non-IDENT byte
// for a closing `>`. A `$<` followed by a malformed IDENT — including
// missing ALPHA, an out-of-charset continuation char, or a missing `>`
// — is literal text.
static bool match_placeholder_lookahead(TSLexer *lexer) {
  if (lexer->lookahead != '$') return false;
  lexer->advance(lexer, false);
  if (lexer->lookahead != '<') return false;
  lexer->advance(lexer, false);
  if (!iswalpha(lexer->lookahead) && lexer->lookahead != '_') return false;
  lexer->advance(lexer, false);
  while (iswalnum(lexer->lookahead) ||
         lexer->lookahead == '_' ||
         lexer->lookahead == '.') {
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead != '>') return false;
  lexer->advance(lexer, false);
  return true;
}

// ── Lua block scanner ──────────────────────────────────────────
// Scans brace-balanced content after `>{`, stopping before the
// closing `}` that balances the opening one. Per CS-0035 / App. A.5,
// braces are inert inside:
//   • double-quoted strings    `"…"`
//   • single-quoted strings    `'…'`
//   • Lua line comments        `-- …` to EOL
//   • leveled long-strings     `[==[ … ]==]` (any `=`-level, multi-line)
//   • leveled block comments   `--[==[ … ]==]` (any `=`-level, multi-line)

// Attempts to consume a leveled long-string opener at the current
// cursor (which MUST be at `[`). Returns the `=`-level (≥ 0) on
// success, leaving the cursor immediately AFTER the opening `[…[`.
// Returns -1 on no match, leaving the cursor unchanged-in-spirit:
// callers that don't want to commit must `mark_end` first.
static int try_long_string_opener(TSLexer *lexer) {
  if (lexer->lookahead != '[') return -1;
  lexer->advance(lexer, false);
  int level = 0;
  while (lexer->lookahead == '=') {
    level++;
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead != '[') return -1;
  lexer->advance(lexer, false);
  return level;
}

// Consumes from the current cursor up to and including the matching
// `]==…]` closer of the given level. Treats every other byte as opaque
// content (including `{` `}` `"` `'`).
static void skip_long_string_body(TSLexer *lexer, int level) {
  while (!lexer->eof(lexer)) {
    if (lexer->lookahead == ']') {
      lexer->advance(lexer, false);
      int eq = 0;
      while (lexer->lookahead == '=') {
        eq++;
        lexer->advance(lexer, false);
      }
      if (eq == level && lexer->lookahead == ']') {
        lexer->advance(lexer, false);
        return;
      }
      // Not a closer — continue scanning, but the `=` chars we
      // consumed are part of the body.
      continue;
    }
    lexer->advance(lexer, false);
  }
}

static bool scan_lua_block_content(TSLexer *lexer) {
  int depth = 0;
  bool has_content = false;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;

    if (c == '{') {
      depth++;
      has_content = true;
      lexer->advance(lexer, false);
    } else if (c == '}') {
      if (depth == 0) {
        lexer->mark_end(lexer);
        lexer->result_symbol = LUA_BLOCK_CONTENT;
        return true;
      }
      depth--;
      has_content = true;
      lexer->advance(lexer, false);
    } else if (c == '"') {
      // Skip double-quoted string
      has_content = true;
      lexer->advance(lexer, false);
      while (!lexer->eof(lexer) && lexer->lookahead != '"') {
        if (lexer->lookahead == '\\')
          lexer->advance(lexer, false);
        if (!lexer->eof(lexer))
          lexer->advance(lexer, false);
      }
      if (!lexer->eof(lexer))
        lexer->advance(lexer, false);
    } else if (c == '\'') {
      // Skip single-quoted string
      has_content = true;
      lexer->advance(lexer, false);
      while (!lexer->eof(lexer) && lexer->lookahead != '\'') {
        if (lexer->lookahead == '\\')
          lexer->advance(lexer, false);
        if (!lexer->eof(lexer))
          lexer->advance(lexer, false);
      }
      if (!lexer->eof(lexer))
        lexer->advance(lexer, false);
    } else if (c == '[') {
      // CS-0035: leveled long-string `[==[ … ]==]` may span newlines.
      // Braces and quotes inside are inert.
      int level = try_long_string_opener(lexer);
      if (level >= 0) {
        has_content = true;
        skip_long_string_body(lexer, level);
      } else {
        // Not a long-string opener — the `[` was consumed by the
        // probe but is otherwise harmless plain content.
        has_content = true;
      }
    } else if (c == '-') {
      has_content = true;
      lexer->advance(lexer, false);
      if (!lexer->eof(lexer) && lexer->lookahead == '-') {
        lexer->advance(lexer, false);
        // CS-0035: distinguish line comment from leveled block comment.
        if (lexer->lookahead == '[') {
          int level = try_long_string_opener(lexer);
          if (level >= 0) {
            skip_long_string_body(lexer, level);
            continue;
          }
          // Fall through: `--[` without a balanced opener is a line
          // comment (the `[` is part of the comment text).
        }
        // Lua line comment — skip to end of line
        while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
          lexer->advance(lexer, false);
        }
      }
    } else {
      has_content = true;
      lexer->advance(lexer, false);
    }
  }

  // EOF without closing brace — emit what we have if non-empty
  if (has_content) {
    lexer->mark_end(lexer);
    lexer->result_symbol = LUA_BLOCK_CONTENT;
    return true;
  }
  return false;
}

// ── Shell block scanner ────────────────────────────────────────
// Scans brace-balanced shell content after `{`, stopping before the
// closing `}` that balances the opening one. Handles shell quoting
// (`"..."`, `'...'`) and `#` line comments so a `{` inside them does
// not affect depth tracking.
//
// CS-0035 (COOK-54): POSIX heredoc opaque-span tracking. After parsing
// a `<<TAG` (or `<<-TAG`, `<<'TAG'`, `<<"TAG"`) opener, the body —
// starting at the next newline — is treated opaquely until the line
// containing the closing TAG. Braces / quotes inside the body are
// inert. State persists across scanner calls so a placeholder on the
// opener line doesn't corrupt heredoc bookkeeping.

// Attempts to parse a heredoc opener at the current cursor (which MUST
// be at the first `<`). Always advances the cursor — caller treats any
// consumed bytes as ordinary body content if the parse fails. Returns
// true and sets `state->in_heredoc = 1` (pending) on a valid opener;
// returns false and leaves `state->in_heredoc = 0` on a malformed
// opener (or just a single `<` redirect).
static bool try_parse_heredoc_opener(TSLexer *lexer, ShellBlockState *state) {
  if (lexer->lookahead != '<') return false;
  lexer->advance(lexer, false);
  if (lexer->lookahead != '<') {
    // Single `<` — likely a redirect operator. Not a heredoc opener.
    return false;
  }
  lexer->advance(lexer, false);
  uint8_t dash = 0;
  if (lexer->lookahead == '-') {
    dash = 1;
    lexer->advance(lexer, false);
  }
  int32_t open_quote = 0;
  if (lexer->lookahead == '\'' || lexer->lookahead == '"') {
    open_quote = lexer->lookahead;
    lexer->advance(lexer, false);
  }
  // TAG = LUA_IDENT-ish. POSIX is broader but the common case is
  // alphanumeric + underscore. Reject anything else as a malformed
  // opener — the consumed `<<` bytes stay as ordinary content.
  if (!(iswalpha(lexer->lookahead) || lexer->lookahead == '_')) {
    return false;
  }
  uint8_t len = 0;
  while (iswalnum(lexer->lookahead) || lexer->lookahead == '_') {
    if (len < HEREDOC_TAG_CAP) {
      state->heredoc_tag[len] = (char)lexer->lookahead;
      len++;
      lexer->advance(lexer, false);
    } else {
      // Tag too long — bail. The bytes we've already consumed stay as
      // ordinary content; the heredoc is not tracked.
      return false;
    }
  }
  if (open_quote != 0) {
    if (lexer->lookahead != open_quote) return false;
    lexer->advance(lexer, false);
  }
  state->in_heredoc = 1;
  state->heredoc_dash = dash;
  state->heredoc_tag_len = len;
  return true;
}

// Consumes a single line of heredoc body and checks whether the line
// is the closing TAG line. Returns true (and resets heredoc state) on
// match; returns false to continue body consumption. Always advances
// at least one line on a non-match; on EOF before a match, advances
// to EOF and returns false.
static bool heredoc_try_close_at_line_start(TSLexer *lexer,
                                            ShellBlockState *state) {
  // POSIX says `<<TAG` requires the closing TAG at column 0; `<<-TAG`
  // strips leading tabs. Cook's SHELL_BLOCK_CONTENT normalises by
  // trimming each line's leading/trailing whitespace (App. A.5), so
  // for the in-body close check we strip leading whitespace
  // unconditionally — the indented `        EOF` style used by every
  // Cookfile fixture has to match.
  while (lexer->lookahead == '\t' || lexer->lookahead == ' ') {
    lexer->advance(lexer, false);
  }
  // Attempt to match TAG.
  uint8_t i = 0;
  while (i < state->heredoc_tag_len &&
         lexer->lookahead == (int32_t)state->heredoc_tag[i]) {
    lexer->advance(lexer, false);
    i++;
  }
  if (i == state->heredoc_tag_len &&
      (lexer->lookahead == '\n' || lexer->eof(lexer))) {
    if (lexer->lookahead == '\n') lexer->advance(lexer, false);
    state->in_heredoc = 0;
    state->heredoc_tag_len = 0;
    state->heredoc_dash = 0;
    return true;
  }
  // Not a closing line — drain the rest of this line. The partial
  // TAG-match bytes are body content (advanced already).
  while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead == '\n') lexer->advance(lexer, false);
  return false;
}

static bool scan_shell_block_content(TSLexer *lexer, ShellBlockState *state) {
  bool has_content = false;
  bool at_line_start = true;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;

    // CS-0035: heredoc body is opaque. At line start, try to match the
    // closing TAG line; otherwise drain the line as opaque content.
    if (state->in_heredoc == 2) {
      has_content = true;
      heredoc_try_close_at_line_start(lexer, state);
      at_line_start = true;
      continue;
    }

    // Inside a shell-quoted string: only the matching close quote, an
    // escape (in double-quoted), or a `$<IDENT>` placeholder is special.
    // `{`/`}` inside a string DO NOT affect block-brace depth.
    if (state->in_string != 0) {
      int32_t close = (state->in_string == 1) ? '"' : '\'';
      if (c == close) {
        state->in_string = 0;
        has_content = true;
        lexer->advance(lexer, false);
        continue;
      }
      if (state->in_string == 1 && c == '\\') {
        // Double-quote escape — advance past `\` and the next char.
        has_content = true;
        lexer->advance(lexer, false);
        if (!lexer->eof(lexer)) lexer->advance(lexer, false);
        continue;
      }
      if (c == '$') {
        lexer->mark_end(lexer);
        if (match_placeholder_lookahead(lexer)) {
          if (has_content) {
            lexer->result_symbol = SHELL_BLOCK_CONTENT;
            return true;
          }
          return false;
        }
        lexer->mark_end(lexer);
        has_content = true;
        continue;
      }
      // Ordinary char inside string.
      has_content = true;
      lexer->advance(lexer, false);
      continue;
    }

    // Outside any string.
    if (c == '{') {
      state->depth++;
      has_content = true;
      at_line_start = false;
      lexer->advance(lexer, false);
    } else if (c == '}') {
      if (state->depth == 0) {
        // Return false (rather than an empty SHELL_BLOCK_CONTENT) so the
        // grammar can match the literal `}`. Emitting a zero-length token
        // inside a `repeat(shell_content | placeholder)` would loop forever.
        if (!has_content) return false;
        lexer->mark_end(lexer);
        lexer->result_symbol = SHELL_BLOCK_CONTENT;
        return true;
      }
      state->depth--;
      has_content = true;
      at_line_start = false;
      lexer->advance(lexer, false);
    } else if (c == '$') {
      // §2.11 placeholder boundary. Mark end at the `$` so the cursor
      // can rewind here if lookahead confirms a placeholder.
      lexer->mark_end(lexer);
      if (match_placeholder_lookahead(lexer)) {
        if (has_content) {
          lexer->result_symbol = SHELL_BLOCK_CONTENT;
          return true;
        }
        return false;
      }
      lexer->mark_end(lexer);
      has_content = true;
      at_line_start = false;
    } else if (c == '"') {
      state->in_string = 1;
      has_content = true;
      at_line_start = false;
      lexer->advance(lexer, false);
    } else if (c == '\'') {
      state->in_string = 2;
      has_content = true;
      at_line_start = false;
      lexer->advance(lexer, false);
    } else if (c == '#' && at_line_start) {
      has_content = true;
      while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
        lexer->advance(lexer, false);
      }
    } else if (c == '<') {
      // Try heredoc opener (`<<TAG`, `<<-TAG`, `<<'TAG'`, `<<"TAG"`).
      // try_parse_heredoc_opener always advances; on failure the bytes
      // are ordinary body content (single `<` redirect, etc.).
      has_content = true;
      at_line_start = false;
      try_parse_heredoc_opener(lexer, state);
    } else if (c == '\n') {
      has_content = true;
      lexer->advance(lexer, false);
      at_line_start = true;
      // Pending heredoc → enter body on this newline.
      if (state->in_heredoc == 1) {
        state->in_heredoc = 2;
      }
    } else if (c == ' ' || c == '\t') {
      has_content = true;
      lexer->advance(lexer, false);
    } else {
      has_content = true;
      at_line_start = false;
      lexer->advance(lexer, false);
    }
  }

  if (has_content) {
    lexer->mark_end(lexer);
    lexer->result_symbol = SHELL_BLOCK_CONTENT;
    return true;
  }
  return false;
}

// ── Shell content scanner ──────────────────────────────────────
// Matches a full line of shell content inside a recipe body.
// Returns false for lines starting with keywords or special
// prefixes, letting the internal lexer handle those.

static bool is_step_keyword(const char *word, int len) {
  return (len == 4 && strncmp(word, "cook", 4) == 0) ||
         (len == 4 && strncmp(word, "test", 4) == 0) ||
         (len == 5 && strncmp(word, "plate", 5) == 0) ||
         (len == 11 && strncmp(word, "ingredients", 11) == 0);
}

// A recipe/chore-body line whose first word is one of these keywords
// (followed by an appropriate delimiter) ends the body per App. A.3
// "Body termination" — the grammar's top-level alternation then
// dispatches the next declaration. `recipe`/`chore`/`probe` require a
// space/tab/quote delimiter; `register` additionally accepts newline/EOF
// for its empty-body form. (`use`/`import`/`config` are also termination
// keywords per spec but must precede the first recipe, so a post-recipe
// occurrence is already a semantic error; they're left out to match the
// pre-existing terminator set.)
static bool is_body_terminator_keyword(const char *word, int len,
                                       int32_t next) {
  bool sep = (next == ' ' || next == '\t' || next == '"');
  if (len == 6 && strcmp(word, "recipe") == 0) return sep;
  if (len == 5 && strcmp(word, "chore") == 0) return sep;
  if (len == 5 && strcmp(word, "probe") == 0) return sep;
  if (len == 8 && strcmp(word, "register") == 0)
    return sep || next == '\n' || next == 0;
  return false;
}

// Forward declaration: scans a `. IDENT_START …` module-call statement
// tail, assuming the leading `LUA_IDENT` has already been consumed and the
// cursor sits at the `.`. Defined alongside scan_top_level_module_call_text.
static bool scan_module_call_tail(TSLexer *lexer);

// Matches a recipe/chore-body line. When `module_call_valid` is set the
// parser also accepts a top-level `module_call` here (a reduce-lookahead
// at the body's end); a column-0 `LUA_IDENT . IDENT_START …` line is then
// recognised as a module_call that terminates the body. CRUCIAL: that
// detection happens INSIDE this single forward pass — reading the leading
// identifier exactly once — rather than via a separate pre-pass. A
// separate `scan_top_level_module_call_text` pre-pass would advance the
// shared lexer cursor past the identifier and, on a non-match (e.g. the
// keyword `recipe`), leave the cursor mid-line for the shell-content scan,
// silently swallowing the keyword and merging two declarations into one.
// That double-scan was the root cause of the body-termination bug.
static bool scan_shell_content(TSLexer *lexer, bool module_call_valid) {
  // Skip leading whitespace — tree-sitter does NOT consume extras
  // before calling external scanners. Track whether any was skipped: a
  // top-level module_call only terminates a body at column 0 (no leading
  // whitespace); an indented `foo.bar()` is recipe-body shell (CS-0072).
  bool at_col0 = true;
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
    at_col0 = false;
  }

  int32_t c = lexer->lookahead;

  // Not shell: empty line, comment, lua prefix, interactive prefix
  if (c == '\n' || c == 0)
    return false;
  if (c == '#' || c == '>' || c == '@')
    return false;
  // Note: a leading `"` is allowed — shell lines may begin with a
  // quoted string (e.g. an executable path with spaces). The earlier
  // `c == '"'` early-bail was there to leave the byte for a `string`
  // token, but `shell_command` is the only rule that requests
  // `_shell_content`, and its alternation has no `string` arm; bailing
  // here would cause `echo "x: $<dep>"` to ERROR after the embedded
  // placeholder because the resumed scan starts at the closing `"`.

  bool has_content = false;

  // If starts with an identifier, check for step keywords, body-
  // termination keywords, and the column-0 module-call dispatch pattern.
  if (iswalpha(c) || c == '_') {
    char word[16];
    int len = 0;
    bool word_truncated = false;

    while ((iswalnum(lexer->lookahead) || lexer->lookahead == '_')) {
      if (len < 15) {
        word[len++] = (char)lexer->lookahead;
      } else {
        word_truncated = true;
      }
      lexer->advance(lexer, false);
    }
    word[len] = '\0';

    int32_t next = lexer->lookahead;

    // Step keywords — when followed by whitespace or quote, yield to the
    // grammar's dedicated step rules.
    if (!word_truncated && is_step_keyword(word, len)) {
      if (next == ' ' || next == '\t' || next == '"')
        return false;
    }

    // Body-termination keywords (recipe/chore/probe/register, App. A.3 /
    // A.3.2). Returning false yields the WHOLE scan, so tree-sitter resets
    // the lexer to the line start and re-lexes the keyword internally; the
    // recipe/chore body then reduces and the top-level alternation
    // dispatches the next declaration.
    if (!word_truncated && is_body_terminator_keyword(word, len, next)) {
      return false;
    }

    // Column-0 top-level module_call (`LUA_IDENT . IDENT_START …`,
    // App. A.4). Detected here, reading the leading IDENT once. A match
    // terminates the body and emits TOP_LEVEL_MODULE_CALL_TEXT; a near-
    // miss (`foo.123`) falls through and the bytes already consumed remain
    // part of the shell-content token. Indented `foo.bar()` is shell
    // (CS-0072), hence the `at_col0` gate.
    if (module_call_valid && at_col0 && !word_truncated && next == '.') {
      if (scan_module_call_tail(lexer)) {
        return true;
      }
    }

    // Reaching here means the alpha branch consumed at least one
    // identifier byte that is part of the shell command.
    has_content = true;
  }

  // Consume rest of line. The `has_content` flag distinguishes a
  // placeholder at the start of a line (yield to grammar, return false)
  // from one mid-line (emit accumulated content, return true).
  while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
    if (lexer->lookahead == '$') {
      // §2.11 placeholder boundary — see the comment above
      // match_placeholder_lookahead for the mark_end pattern.
      lexer->mark_end(lexer);
      if (match_placeholder_lookahead(lexer)) {
        if (has_content) {
          lexer->result_symbol = SHELL_CONTENT;
          return true;
        }
        return false;
      }
      lexer->mark_end(lexer);
      has_content = true;
      continue;
    }
    lexer->advance(lexer, false);
    has_content = true;
  }

  lexer->mark_end(lexer);
  lexer->result_symbol = SHELL_CONTENT;
  return has_content;
}

// ── Top-level keyword check ────────────────────────────────────
// Returns true if the buffer (length len) matches a top-level Cookfile
// keyword that implicitly terminates a config/register body
// (§{toplevel.termination}, App. A.2): recipe, chore, probe, config,
// use, import, register. The `register` keyword joins this set per
// CS-0072; `probe` joins per COOK-67 (App. A.3.2).

static bool is_toplevel_keyword(const char *buf, int len) {
  return (len == 6 && strncmp(buf, "recipe", 6) == 0) ||
         (len == 5 && strncmp(buf, "chore", 5) == 0) ||
         (len == 5 && strncmp(buf, "probe", 5) == 0) ||
         (len == 6 && strncmp(buf, "config", 6) == 0) ||
         (len == 3 && strncmp(buf, "use", 3) == 0) ||
         (len == 6 && strncmp(buf, "import", 6) == 0) ||
         (len == 8 && strncmp(buf, "register", 8) == 0);
}

// ── Top-level Lua source scanner ───────────────────────────────
// Shared scan for `config NAME\n…` and `register\n…` bodies. Scans
// the Lua content up to the next column-0 top-level keyword (recipe,
// chore, config, use, import, register) or EOF; stops before the
// keyword line, leaving it for the grammar to consume. Handles
// strings/comments so keywords inside them don't terminate the body.
// CS-0072: `register` joins the toplevel keyword set so register
// blocks split from config and from each other. The two callers
// distinguish via the `result_symbol` argument.
//
static bool scan_lua_source_until_toplevel(TSLexer *lexer,
                                           enum TokenType result_symbol) {
  bool has_content = false;
  bool at_line_start = true;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;

    if (at_line_start) {
      // At column 0: check for a top-level keyword that would start a
      // new top-level declaration. We do NOT skip leading whitespace
      // here — column-0 means the very first character of the line.
      if (c != ' ' && c != '\t' && c != '\n' && c != 0) {
        // A column-0 `#` is a Cookfile-level comment, not Lua body.
        // Stop here so the grammar can match it as a top-level comment.
        // (Lua uses `--` for comments; column-0 `#` would be invalid Lua,
        // so this carries no risk of swallowing real config-body content.)
        if (c == '#') {
          lexer->mark_end(lexer);
          lexer->result_symbol = result_symbol;
          return has_content;
        }
        // Peek ahead to read an identifier. The buffer holds up to 15
        // chars; longer LUA_IDENTs will overflow the buffer harmlessly
        // (the keyword check below won't match any 8+ char keyword).
        char word[16];
        int len = 0;
        // Mark position before consuming any chars
        lexer->mark_end(lexer);
        if (iswalpha(lexer->lookahead) || lexer->lookahead == '_') {
          while (iswalnum(lexer->lookahead) || lexer->lookahead == '_') {
            if (len < 15) {
              word[len++] = (char)lexer->lookahead;
            }
            lexer->advance(lexer, false);
          }
        }
        word[len] = '\0';
        int32_t after = lexer->lookahead;
        // CS-0072: top-level `module_call` shape — `LUA_IDENT . IDENT_START`
        // at column 0 — terminates the surrounding register / config body
        // so the grammar's `top_level_module_call` can be matched next.
        // Check this BEFORE the keyword check so `register.foo()` parses
        // as a top-level module_call, not as a (truncated) `register`
        // keyword followed by Lua body content.
        if (len > 0 && after == '.') {
          lexer->advance(lexer, false);
          int32_t after_dot = lexer->lookahead;
          if (iswalpha(after_dot) || after_dot == '_') {
            lexer->result_symbol = result_symbol;
            return has_content;
          }
          // Not a module_call shape (`foo.123`, `foo.-x`, …). We've
          // consumed past the dot; fall through to ordinary body.
          has_content = true;
          at_line_start = false;
          continue;
        }
        // Check if it's a top-level keyword followed by whitespace/newline/EOF
        if (is_toplevel_keyword(word, len) &&
            (after == ' ' || after == '\t' || after == '\n' || after == 0 || after == '"')) {
          // Found top-level keyword at column 0 — stop before it
          lexer->result_symbol = result_symbol;
          return has_content;
        }
        // Not a top-level keyword — we've already advanced past some chars
        has_content = true;
        at_line_start = false;
        continue;
      }
      at_line_start = false;
    }

    if (c == '\n') {
      has_content = true;
      lexer->advance(lexer, false);
      at_line_start = true;
      continue;
    }

    if (c == '"') {
      // Skip double-quoted string
      has_content = true;
      lexer->advance(lexer, false);
      while (!lexer->eof(lexer) && lexer->lookahead != '"' && lexer->lookahead != '\n') {
        if (lexer->lookahead == '\\')
          lexer->advance(lexer, false);
        if (!lexer->eof(lexer))
          lexer->advance(lexer, false);
      }
      if (!lexer->eof(lexer) && lexer->lookahead == '"')
        lexer->advance(lexer, false);
      continue;
    }

    if (c == '\'') {
      // Skip single-quoted string
      has_content = true;
      lexer->advance(lexer, false);
      while (!lexer->eof(lexer) && lexer->lookahead != '\'' && lexer->lookahead != '\n') {
        if (lexer->lookahead == '\\')
          lexer->advance(lexer, false);
        if (!lexer->eof(lexer))
          lexer->advance(lexer, false);
      }
      if (!lexer->eof(lexer) && lexer->lookahead == '\'')
        lexer->advance(lexer, false);
      continue;
    }

    if (c == '-') {
      has_content = true;
      lexer->advance(lexer, false);
      if (!lexer->eof(lexer) && lexer->lookahead == '-') {
        while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
          lexer->advance(lexer, false);
        }
      }
      continue;
    }

    has_content = true;
    lexer->advance(lexer, false);
  }

  // EOF without `end` — emit what we have
  if (has_content) {
    lexer->mark_end(lexer);
    lexer->result_symbol = result_symbol;
    return true;
  }
  return false;
}

// ── Top-level module_call shape predicate ──────────────────────
// Returns true iff the bytes at the current cursor look like the
// start of a top-level `module_call`: `LUA_IDENT . IDENT_START`. The
// LUA_IDENT (`/[A-Za-z_][A-Za-z0-9_]*/`) is strict — hyphens and dots
// are NOT permitted in the first segment per App. A.4. Consumes the
// inspected bytes via `advance`; callers MUST `mark_end` before
// invoking so they can rewind on a miss.
static bool peek_top_level_module_call_shape(TSLexer *lexer) {
  int32_t c = lexer->lookahead;
  if (!iswalpha(c) && c != '_') return false;
  lexer->advance(lexer, false);
  while (iswalnum(lexer->lookahead) || lexer->lookahead == '_') {
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead != '.') return false;
  lexer->advance(lexer, false);
  c = lexer->lookahead;
  return iswalpha(c) || c == '_';
}

// ── Top-level module_call scanner ──────────────────────────────
// Consumes a column-0 `LUA_IDENT . IDENT_START …` statement, ending
// at a newline encountered with brace_depth == 0. Multi-line forms
// (App. A.4 + § 2.9) brace-balance using the same opaque-span rules
// as `scan_lua_block_content`: strings, single-line comments, and
// (TODO: issue COOK-53) leveled long-strings / block comments are
// inert. Parentheses are NOT balanced — only braces matter for
// statement extent.
//
// Activation conditions:
//   • valid_symbols[TOP_LEVEL_MODULE_CALL_TEXT] is set, AND
//   • the cursor is at column 0 (the grammar reaches this only at
//     toplevel position, so this is guaranteed by structure), AND
//   • the bytes match `LUA_IDENT . IDENT_START` (see peek above).
static bool scan_top_level_module_call_text(TSLexer *lexer) {
  // Verify the shape opens here. Note: the lexer's column at entry
  // is wherever the grammar called us from — `_toplevel_item` is only
  // reached at column 0 in this grammar, so we don't need an explicit
  // column check.
  if (!iswalpha(lexer->lookahead) && lexer->lookahead != '_') return false;
  // Consume LUA_IDENT.
  while (iswalnum(lexer->lookahead) || lexer->lookahead == '_') {
    lexer->advance(lexer, false);
  }
  return scan_module_call_tail(lexer);
}

// Scans the `. IDENT_START …` tail of a `module_call`. Precondition: the
// leading `LUA_IDENT` has already been consumed and `lexer->lookahead` is
// the byte immediately after it. Returns true (result_symbol =
// TOP_LEVEL_MODULE_CALL_TEXT) when a well-formed statement tail follows;
// otherwise false. On a false return the cursor MAY have advanced past the
// `.` — callers that fall back to another token kind tolerate this because
// the consumed bytes stay within the eventual token's [start, mark_end]
// span (see scan_shell_content).
static bool scan_module_call_tail(TSLexer *lexer) {
  if (lexer->lookahead != '.') return false;
  lexer->advance(lexer, false);
  if (!iswalpha(lexer->lookahead) && lexer->lookahead != '_') return false;
  // Consume the rest of the statement.
  int brace_depth = 0;
  bool in_dq = false;     // double-quoted string
  bool in_sq = false;     // single-quoted string
  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;
    if (in_dq) {
      if (c == '\\') {
        lexer->advance(lexer, false);
        if (!lexer->eof(lexer)) lexer->advance(lexer, false);
        continue;
      }
      if (c == '"') { in_dq = false; }
      lexer->advance(lexer, false);
      continue;
    }
    if (in_sq) {
      if (c == '\\') {
        lexer->advance(lexer, false);
        if (!lexer->eof(lexer)) lexer->advance(lexer, false);
        continue;
      }
      if (c == '\'') { in_sq = false; }
      lexer->advance(lexer, false);
      continue;
    }
    if (c == '"') { in_dq = true; lexer->advance(lexer, false); continue; }
    if (c == '\'') { in_sq = true; lexer->advance(lexer, false); continue; }
    if (c == '-') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '-') {
        // Lua line comment to end of line — but a comment outside braces
        // means we're past the statement at the next newline anyway, so
        // just skip to EOL.
        while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
          lexer->advance(lexer, false);
        }
      }
      continue;
    }
    if (c == '{') { brace_depth++; lexer->advance(lexer, false); continue; }
    if (c == '}') {
      if (brace_depth > 0) brace_depth--;
      lexer->advance(lexer, false);
      continue;
    }
    if (c == '\n') {
      if (brace_depth == 0) {
        // End of statement. Newline is NOT consumed — it's the grammar's
        // `_newline` terminator.
        lexer->mark_end(lexer);
        lexer->result_symbol = TOP_LEVEL_MODULE_CALL_TEXT;
        return true;
      }
      lexer->advance(lexer, false);
      continue;
    }
    lexer->advance(lexer, false);
  }
  // EOF with closed braces — still a valid statement.
  if (brace_depth == 0) {
    lexer->mark_end(lexer);
    lexer->result_symbol = TOP_LEVEL_MODULE_CALL_TEXT;
    return true;
  }
  return false;
}

// ── Step-pattern continuation newline (CS-0078) ────────────────
// Emitted between successive `STRING` (or `!STRING`) tokens in
// `cook_step` and `ingredients_step` when the next pattern lives on
// a subsequent line. Per App. A.4: continuation lines beginning with
// `"` (or `!"` for `ingredients`) extend the same declaration. A
// non-quote first token on the next line terminates the declaration
// silently — the scanner returns false in that case and the grammar
// dispatches per App. A.4's step-priority order.
//
// The valid_symbols gate ensures this is only attempted when the
// grammar expects more patterns; outside of that position the bare
// `_newline` rule consumes the newline as usual.
static bool scan_step_continuation_newline(TSLexer *lexer) {
  if (lexer->lookahead != '\n') return false;
  // Consume the newline + any further blank lines and leading
  // whitespace on the continuation line.
  while (lexer->lookahead == '\n' || lexer->lookahead == ' ' ||
         lexer->lookahead == '\t') {
    lexer->advance(lexer, false);
  }
  int32_t c = lexer->lookahead;
  if (c == '"') {
    // Token spans newline + leading-WS. The `"` itself is the start of
    // the next STRING and is NOT included.
    lexer->mark_end(lexer);
    lexer->result_symbol = STEP_CONTINUATION_NEWLINE;
    return true;
  }
  if (c == '!') {
    // Mark before consuming `!` so the grammar sees the `!` next as
    // the start of an ingredient_exclude. We need a second char of
    // lookahead — peek by advancing.
    lexer->mark_end(lexer);
    lexer->advance(lexer, false);
    if (lexer->lookahead == '"') {
      lexer->result_symbol = STEP_CONTINUATION_NEWLINE;
      return true;
    }
    return false;
  }
  return false;
}

// ── External scanner API ───────────────────────────────────────

void *tree_sitter_cook_external_scanner_create(void) {
  ShellBlockState *state = calloc(1, sizeof(ShellBlockState));
  return state;
}

void tree_sitter_cook_external_scanner_destroy(void *payload) {
  free(payload);
}

// Byte layout (versioned by length so older serialised payloads still
// load gracefully):
//   [0]      depth
//   [1]      in_string
//   [2]      in_heredoc            (CS-0035 addition)
//   [3]      heredoc_dash          (CS-0035 addition)
//   [4]      heredoc_tag_len       (CS-0035 addition)
//   [5..5+N] heredoc_tag bytes (length = heredoc_tag_len)
unsigned tree_sitter_cook_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  ShellBlockState *state = (ShellBlockState *)payload;
  buffer[0] = (char)state->depth;
  buffer[1] = (char)state->in_string;
  buffer[2] = (char)state->in_heredoc;
  buffer[3] = (char)state->heredoc_dash;
  buffer[4] = (char)state->heredoc_tag_len;
  unsigned n = 5;
  for (unsigned i = 0; i < state->heredoc_tag_len && i < HEREDOC_TAG_CAP; i++) {
    buffer[n++] = state->heredoc_tag[i];
  }
  return n;
}

void tree_sitter_cook_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {
  ShellBlockState *state = (ShellBlockState *)payload;
  // Zero-init all fields so leftover bytes from an older layout don't
  // leak through.
  state->depth = 0;
  state->in_string = 0;
  state->in_heredoc = 0;
  state->heredoc_dash = 0;
  state->heredoc_tag_len = 0;
  for (unsigned i = 0; i < HEREDOC_TAG_CAP; i++) state->heredoc_tag[i] = 0;
  if (length >= 2) {
    state->depth = (uint8_t)buffer[0];
    state->in_string = (uint8_t)buffer[1];
  }
  if (length >= 5) {
    state->in_heredoc = (uint8_t)buffer[2];
    state->heredoc_dash = (uint8_t)buffer[3];
    state->heredoc_tag_len = (uint8_t)buffer[4];
    unsigned cap = state->heredoc_tag_len < HEREDOC_TAG_CAP
                   ? state->heredoc_tag_len : HEREDOC_TAG_CAP;
    for (unsigned i = 0; i < cap && (5 + i) < length; i++) {
      state->heredoc_tag[i] = buffer[5 + i];
    }
  }
}

bool tree_sitter_cook_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  ShellBlockState *state = (ShellBlockState *)payload;

  // Pure top-level module_call (between declarations, where SHELL_CONTENT
  // is NOT valid). Inside a recipe/chore body BOTH this and SHELL_CONTENT
  // are valid reduce-lookaheads; there, module-call detection is folded
  // into scan_shell_content so the leading identifier is read only once
  // (a separate pre-pass here would advance the cursor and corrupt the
  // shell-content scan — the body-termination bug).
  if (valid_symbols[TOP_LEVEL_MODULE_CALL_TEXT] && !valid_symbols[SHELL_CONTENT]) {
    if (scan_top_level_module_call_text(lexer)) {
      return true;
    }
    // Fall through — other top-level tokens may also be valid.
  }

  if (valid_symbols[STEP_CONTINUATION_NEWLINE]) {
    if (scan_step_continuation_newline(lexer)) {
      return true;
    }
    // Fall through — the bare _newline rule will consume the newline.
  }

  if (valid_symbols[REGISTER_BLOCK_CONTENT]) {
    return scan_lua_source_until_toplevel(lexer, REGISTER_BLOCK_CONTENT);
  }

  if (valid_symbols[CONFIG_BLOCK_CONTENT]) {
    return scan_lua_source_until_toplevel(lexer, CONFIG_BLOCK_CONTENT);
  }

  if (valid_symbols[SHELL_BLOCK_CONTENT]) {
    return scan_shell_block_content(lexer, state);
  }

  if (valid_symbols[LUA_BLOCK_CONTENT]) {
    return scan_lua_block_content(lexer);
  }

  if (valid_symbols[SHELL_CONTENT]) {
    return scan_shell_content(lexer, valid_symbols[TOP_LEVEL_MODULE_CALL_TEXT]);
  }

  return false;
}
