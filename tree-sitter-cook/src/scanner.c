#include "tree_sitter/parser.h"

#include <wctype.h>
#include <stdbool.h>
#include <string.h>

enum TokenType {
  LUA_BLOCK_CONTENT,
  SHELL_CONTENT,
  CONFIG_BLOCK_CONTENT,
  SHELL_BLOCK_CONTENT,
};

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

static bool scan_shell_block_content(TSLexer *lexer) {
  int depth = 0;
  bool has_content = false;
  bool at_line_start = true;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;

    if (c == '{') {
      depth++;
      has_content = true;
      at_line_start = false;
      lexer->advance(lexer, false);
    } else if (c == '}') {
      if (depth == 0) {
        lexer->mark_end(lexer);
        lexer->result_symbol = SHELL_BLOCK_CONTENT;
        return true;
      }
      depth--;
      has_content = true;
      at_line_start = false;
      lexer->advance(lexer, false);
    } else if (c == '"') {
      has_content = true;
      at_line_start = false;
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
      has_content = true;
      at_line_start = false;
      lexer->advance(lexer, false);
      while (!lexer->eof(lexer) && lexer->lookahead != '\'') {
        if (!lexer->eof(lexer))
          lexer->advance(lexer, false);
      }
      if (!lexer->eof(lexer))
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

    // `recipe` keyword — explicit recipe shouldn't appear inside body,
    // but let the internal lexer handle it for error recovery
    if (!word_truncated && len == 6 && strcmp(word, "recipe") == 0) {
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
  }

  // Consume rest of line
  while (!lexer->eof(lexer) && lexer->lookahead != '\n') {
    lexer->advance(lexer, false);
  }

  lexer->mark_end(lexer);
  lexer->result_symbol = SHELL_CONTENT;
  return true;
}

// ── Top-level keyword check ────────────────────────────────────
// Returns true if the buffer (length len) matches a top-level Cookfile
// keyword: recipe, config, use, import.

static bool is_toplevel_keyword(const char *buf, int len) {
  return (len == 6 && strncmp(buf, "recipe", 6) == 0) ||
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

void *tree_sitter_cook_external_scanner_create(void) { return NULL; }

void tree_sitter_cook_external_scanner_destroy(void *payload) {}

unsigned tree_sitter_cook_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  return 0;
}

void tree_sitter_cook_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {}

bool tree_sitter_cook_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  if (valid_symbols[CONFIG_BLOCK_CONTENT]) {
    return scan_config_block_content(lexer);
  }

  if (valid_symbols[SHELL_BLOCK_CONTENT]) {
    return scan_shell_block_content(lexer);
  }

  if (valid_symbols[LUA_BLOCK_CONTENT]) {
    return scan_lua_block_content(lexer);
  }

  if (valid_symbols[SHELL_CONTENT]) {
    return scan_shell_content(lexer);
  }

  return false;
}
