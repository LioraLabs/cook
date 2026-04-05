#include "tree_sitter/parser.h"

#include <ctype.h>
#include <stdbool.h>
#include <string.h>

enum TokenType {
  LUA_BLOCK_CONTENT,
  SHELL_CONTENT,
  CONFIG_BLOCK_CONTENT,
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

  // If starts with an identifier, check for step keywords and `end`
  if (isalpha(c) || c == '_') {
    char word[16];
    int len = 0;

    while ((isalnum(lexer->lookahead) || lexer->lookahead == '_') &&
           len < 15) {
      word[len++] = (char)lexer->lookahead;
      lexer->advance(lexer, false);
    }
    word[len] = '\0';

    int32_t next = lexer->lookahead;

    // `end` keyword — only when it's the entire line (followed by newline/EOF)
    if (len == 3 && strcmp(word, "end") == 0) {
      if (next == '\n' || next == 0 || next == ' ' || next == '\t')
        return false;
    }

    // Step keywords — when followed by whitespace or quote
    if (is_step_keyword(word, len)) {
      if (next == ' ' || next == '\t' || next == '"')
        return false;
    }

    // `recipe` keyword — explicit recipe shouldn't appear inside body,
    // but let the internal lexer handle it for error recovery
    if (len == 6 && strcmp(word, "recipe") == 0) {
      if (next == ' ' || next == '\t' || next == '"')
        return false;
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

// ── Config block scanner ───────────────────────────────────────
// Scans Lua content between `config NAME\n` and `end` (on its own
// line). Stops before the `end` keyword, leaving it for the grammar
// to consume. Handles strings/comments so `end` inside them doesn't
// terminate the body.

static bool scan_config_block_content(TSLexer *lexer) {
  bool has_content = false;
  bool at_line_start = true;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;

    if (at_line_start) {
      // Skip leading whitespace on this line
      while (c == ' ' || c == '\t') {
        lexer->advance(lexer, false);
        c = lexer->lookahead;
      }
      // Check for `end` as the entire line
      if (c == 'e') {
        // Peek ahead for `end` followed by newline/EOF
        lexer->mark_end(lexer);
        lexer->advance(lexer, false);
        if (lexer->lookahead == 'n') {
          lexer->advance(lexer, false);
          if (lexer->lookahead == 'd') {
            lexer->advance(lexer, false);
            int32_t after = lexer->lookahead;
            if (after == '\n' || after == 0 || after == ' ' || after == '\t' || after == '\r') {
              // Found `end` on its own line — stop before it
              lexer->result_symbol = CONFIG_BLOCK_CONTENT;
              return has_content;
            }
          }
        }
        // Not `end` — we've already advanced past some chars; the
        // partial content is already accumulated via has_content.
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

  if (valid_symbols[LUA_BLOCK_CONTENT]) {
    return scan_lua_block_content(lexer);
  }

  if (valid_symbols[SHELL_CONTENT]) {
    return scan_shell_content(lexer);
  }

  return false;
}
