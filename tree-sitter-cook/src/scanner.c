#include "tree_sitter/parser.h"

#include <ctype.h>
#include <stdbool.h>
#include <string.h>

enum TokenType {
  LUA_BLOCK_CONTENT,
  SHELL_CONTENT,
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
  if (valid_symbols[LUA_BLOCK_CONTENT]) {
    return scan_lua_block_content(lexer);
  }

  if (valid_symbols[SHELL_CONTENT]) {
    return scan_shell_content(lexer);
  }

  return false;
}
