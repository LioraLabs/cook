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
};

// Persistent state for the SHELL_BLOCK_CONTENT scanner. A `$<IDENT>`
// placeholder inside a shell-quoted string ("...$<X>...") splits the
// scan across multiple calls; without persisting the in-string state,
// the resumed scan would mistreat the closing `"` as opening a new
// string and the trailing `}` would land in the wrong context.
typedef struct {
  uint8_t depth;
  uint8_t in_string;  // 0 = outside, 1 = double-quoted, 2 = single-quoted
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
// closing `}` that balances the opening one.

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
    } else if (c == '-') {
      has_content = true;
      lexer->advance(lexer, false);
      if (!lexer->eof(lexer) && lexer->lookahead == '-') {
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

static bool scan_shell_block_content(TSLexer *lexer, ShellBlockState *state) {
  bool has_content = false;
  bool at_line_start = true;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;

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
    } else if (c == '\n') {
      has_content = true;
      lexer->advance(lexer, false);
      at_line_start = true;
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

static bool scan_shell_content(TSLexer *lexer) {
  // Skip leading whitespace — tree-sitter does NOT consume extras
  // before calling external scanners.
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
  }

  int32_t c = lexer->lookahead;

  // Not shell: empty line, comment, lua prefix, interactive prefix
  if (c == '\n' || c == 0)
    return false;
  if (c == '#' || c == '>' || c == '@')
    return false;
  // Not shell: quoted string (would be handled by string token)
  if (c == '"')
    return false;

  bool has_content = false;

  // If starts with an identifier, check for step keywords, `end`, and
  // the module-call dispatch pattern (App. A.4).
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

    // Step keywords — when followed by whitespace or quote
    if (!word_truncated && is_step_keyword(word, len)) {
      if (next == ' ' || next == '\t' || next == '"')
        return false;
    }

    // `recipe` / `chore` keyword — explicit recipe/chore shouldn't appear
    // inside a body; let the internal lexer handle it for error recovery
    if (!word_truncated && len == 6 && strcmp(word, "recipe") == 0) {
      if (next == ' ' || next == '\t' || next == '"')
        return false;
    }
    if (!word_truncated && len == 5 && strcmp(word, "chore") == 0) {
      if (next == ' ' || next == '\t' || next == '"')
        return false;
    }

    // Module-call dispatch (App. A.4): the first segment is a bare
    // alphanumeric+underscore identifier (no dots, no hyphens), then a
    // literal `.`, then a character matching ident-start. If both
    // hold, defer to the grammar's `module_call` rule by refusing.
    if (next == '.') {
      lexer->advance(lexer, false);
      int32_t after_dot = lexer->lookahead;
      if (iswalpha(after_dot) || after_dot == '_') {
        return false;
      }
      // Otherwise (`foo.123`, `foo.-x`, `foo.`) fall through and
      // consume the rest of the line as a shell command.
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
// keyword: recipe, config, use, import.

static bool is_toplevel_keyword(const char *buf, int len) {
  return (len == 6 && strncmp(buf, "recipe", 6) == 0) ||
         (len == 5 && strncmp(buf, "chore", 5) == 0) ||
         (len == 6 && strncmp(buf, "config", 6) == 0) ||
         (len == 3 && strncmp(buf, "use", 3) == 0) ||
         (len == 6 && strncmp(buf, "import", 6) == 0);
}

// ── Config block scanner ───────────────────────────────────────
// Scans Lua content between `config NAME\n` and the next column-0
// top-level keyword (recipe, config, use, import) or EOF. Stops
// before the top-level keyword line, leaving it for the grammar to
// consume. Handles strings/comments so keywords inside them don't
// terminate the body.

static bool scan_config_block_content(TSLexer *lexer) {
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
          lexer->result_symbol = CONFIG_BLOCK_CONTENT;
          return has_content;
        }
        // Peek ahead to read an identifier
        char word[8];
        int len = 0;
        // Mark position before consuming any chars
        lexer->mark_end(lexer);
        while (len < 7 && (iswalpha(lexer->lookahead) || lexer->lookahead == '_')) {
          word[len++] = (char)lexer->lookahead;
          lexer->advance(lexer, false);
        }
        word[len] = '\0';
        int32_t after = lexer->lookahead;
        // Check if it's a top-level keyword followed by whitespace/newline/EOF
        if (is_toplevel_keyword(word, len) &&
            (after == ' ' || after == '\t' || after == '\n' || after == 0 || after == '"')) {
          // Found top-level keyword at column 0 — stop before it
          lexer->result_symbol = CONFIG_BLOCK_CONTENT;
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
    lexer->result_symbol = CONFIG_BLOCK_CONTENT;
    return true;
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

unsigned tree_sitter_cook_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  ShellBlockState *state = (ShellBlockState *)payload;
  buffer[0] = (char)state->depth;
  buffer[1] = (char)state->in_string;
  return 2;
}

void tree_sitter_cook_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {
  ShellBlockState *state = (ShellBlockState *)payload;
  if (length >= 2) {
    state->depth = (uint8_t)buffer[0];
    state->in_string = (uint8_t)buffer[1];
  } else {
    state->depth = 0;
    state->in_string = 0;
  }
}

bool tree_sitter_cook_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  ShellBlockState *state = (ShellBlockState *)payload;

  if (valid_symbols[CONFIG_BLOCK_CONTENT]) {
    return scan_config_block_content(lexer);
  }

  if (valid_symbols[SHELL_BLOCK_CONTENT]) {
    return scan_shell_block_content(lexer, state);
  }

  if (valid_symbols[LUA_BLOCK_CONTENT]) {
    return scan_lua_block_content(lexer);
  }

  if (valid_symbols[SHELL_CONTENT]) {
    return scan_shell_content(lexer);
  }

  return false;
}
