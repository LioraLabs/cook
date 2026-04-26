# Remove VarDecl in Favor of Config-Block-Only Variables — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the design in `standard/specs/2026-04-26-remove-vardecl-design.md` to the Cook Standard. After this plan executes, the Standard no longer describes a top-level `NAME "value"` form, defines a normative composition rule for `config` blocks (base + at most one named overlay, last-write-wins), binds `env` as a normative alias for `cook.env` inside config bodies, and records the change as CS-0011 in Appendix D.

**Architecture:** Spec-only edit pass over the Astro-Starlight MDX sources under `standard/src/content/docs/`. Each task is a coherent, revertable commit affecting one chapter (or one rationale appendix). The Astro build (with `rehype-bare-ref-lint`, `rehype-clause-anchors`, `remark-rfc2119`) gates every commit; the bash keyword lint and the vitest suite run alongside. No CLI/parser/tree-sitter changes — those are deferred to a follow-up CS that will pair the parser implementation with the new conformance fixtures for §3.6.1.

**Tech Stack:** MDX (Astro Starlight), bash (`scripts/check-normative-keywords.sh`), pnpm + vitest, slug-based cross-references via the custom `remark-slug-xrefs` and `rehype-bare-ref-lint` plugins.

---

## Working directory and prerequisites

All paths are relative to `/home/alex/dev/cook` unless noted.

Before starting, run once at the repo root to confirm the spec-first hook is installed (it should already be):

```bash
git -C /home/alex/dev/cook config --get core.hooksPath
# Expected: .githooks
```

If the output is empty, run `git -C /home/alex/dev/cook config core.hooksPath .githooks` once.

The plan touches only files under `standard/`, so the spec-first hook will not trigger any "language change without spec update" warnings.

## Per-task verification commands

Each task ends with the same verification sequence before committing. Reproduced here so individual tasks stay short:

```bash
cd /home/alex/dev/cook/standard
pnpm build              # Astro build with rehype-bare-ref-lint and rehype-clause-anchors
pnpm test               # vitest plugin tests
pnpm lint:keywords      # RFC-2119 lowercase-keyword lint over normative chapters
```

Expected for all three: clean exit (status 0) with no errors. `pnpm build` may warn about asset sizes; only `error` level lines are blocking.

---

## File structure

Files modified in this plan, by responsibility:

| File | Responsibility | Tasks |
|---|---|---|
| `standard/src/content/docs/02-lexical.mdx` | Token classes, identifier roles, keyword reservation, line-classification cascade, lexical examples. Drop VarDecl token and the blocking-keyword half of §2.4. | Task 1 |
| `standard/src/content/docs/03-syntactic-grammar.mdx` | Top-level prose grammar. Delete §3.3 (variable_declaration), update §3.1/§3.2/§3.8 to drop references, insert new §3.6.1 (composition). | Tasks 2, 5 |
| `standard/src/content/docs/04-recipes.mdx` | Recipe step kinds. Delete Note 4.4.2 (VarDecl reclassification reference). | Task 3 |
| `standard/src/content/docs/06-cook-lua-api.mdx` | Cook Lua API. Add normative `env` alias paragraph. | Task 6 |
| `standard/src/content/docs/appendix/A-grammar.mdx` | Formal EBNF and grammar notes. Remove `variable_declaration` production, drop top-level alternation entry, update grammar comments. | Task 4 |
| `standard/src/content/docs/appendix/B-rationale.mdx` | Informative rationale. Rewrite B.2.4 (drop VarDecl half), delete B.3.8, add new B.3.9 ("Config blocks as the sole variable surface"). | Task 7 |
| `standard/src/content/docs/appendix/D-changes.mdx` | Changelog. Add CS-0011 entry. | Task 8 |

No new files. No deletions of whole files. No conformance fixtures added or removed (fixture work is part of the follow-up parser CS).

---

## Task 1: Drop VarDecl from §2 (lexical layer)

**Files:**
- Modify: `standard/src/content/docs/02-lexical.mdx`

This task removes the lexical-layer description of `VarDecl`: the token row in §2.1, the variable-name role in §2.3, the blocking-keyword half of §2.4, the `variable_declaration` mention in §2.5, line-classification test 11 in §2.10, and Example 2.10.1 + Note 2.10.1.

- [ ] **Step 1.1: Drop the `VarDecl` row from the §2.1 token table**

In `standard/src/content/docs/02-lexical.mdx`, remove the table row for `VarDecl`. The exact line to delete:

```
| `VarDecl`              | Top-level `BARE_IDENTIFIER STRING`                                            | §{grammar.var-declarations}      |
```

After deletion, the table goes directly from the `ConfigHeader` row to the `UseDecl` row.

- [ ] **Step 1.2: Drop the variable-name row from the §2.3 supplementary-constraints table**

Remove the row:

```
| Variable name (`variable_declaration`) | MUST NOT be any of the reserved keywords listed in §{lexical.keywords} (App. A.2).          |
```

After deletion, the §2.3 table starts with the `Import alias` row.

- [ ] **Step 1.3: Rewrite §2.4 ("Keywords") to remove the blocking-keyword set**

Replace the entire §2.4 section content (from the `## 2.4. Keywords [#lexical.keywords]` header through the end of Note 2.4.1, line ~107 in the current file) with:

```mdx
## 2.4. Keywords [#lexical.keywords]
The Cookfile language reserves a small set of identifiers in two roles. Reservation is **contextual**: each identifier is reserved only in the role below, not as a general identifier.

| Reserved recipe segment | Role where reserved             |
|---|---|
| `stem`                 | Final segment of a recipe name   |
| `name`                 | Final segment of a recipe name   |
| `ext`                  | Final segment of a recipe name   |
| `dir`                  | Final segment of a recipe name   |
| `in`                   | Final segment of a recipe name   |
| `out`                  | Final segment of a recipe name   |
| `all`                  | Final segment of a recipe name   |

A conforming implementation MUST reject a recipe whose name's final dot-separated segment is a reserved recipe segment (App. A.2). A reserved recipe segment MAY appear as a non-final segment — for example, `backend.build` is valid because `build` is the final segment, whereas `backend.stem` is rejected because `stem` is the final segment.

The keywords that introduce other top-level or recipe-body productions — `recipe`, `config`, `end`, `ingredients`, `cook`, `plate`, `using`, `use`, `import`, `test` — are recognised as keywords only when followed by a keyword separator (space, tab, or `"`). A `Content` line whose first word is one of these keywords followed directly by other identifier characters (for example, `cooking`, `testify`) is a `shell_command`. The dispatch rule is normative in App. A.4.

### Note 2.4.1

The separator test is the governing subtlety. See the test `test_recipe_prefix_is_shell_command` in `cli/crates/cook-lang/src/lexer.rs`, which confirms that `recipes_cleanup` produces a `Content` token and not a `RecipeHeader`.
```

The blocking-keyword table is gone; the closing paragraph is rephrased to motivate the keyword-separator rule on its own (the only surviving consumer of the keyword-as-keyword pattern). Note 2.4.1 is unchanged.

- [ ] **Step 1.4: Update §2.5 prose to drop the `variable_declaration` mention**

In §2.5 ("Strings"), find the paragraph beginning `STRING appears in top-level variable_declaration ...`. Replace:

```
`STRING` appears in top-level `variable_declaration` (the value), in recipe and dependency names, in `ingredients`, `cook`, `plate`, and `test` steps, and as the single-string form of `using_clause`. In each of those roles, a conforming implementation parses the `STRING` inside the single line that produced the governing `Content` token.
```

with:

```
`STRING` appears in recipe and dependency names, in `ingredients`, `cook`, `plate`, and `test` steps, in the optional name of a `config_block` header, and as the single-string form of `using_clause`. In each of those roles, a conforming implementation parses the `STRING` inside the single line that produced the governing token.
```

Note: the `config_block` name option is added in place of the deleted variable-declaration mention so the inventory of `STRING` consumers stays complete.

- [ ] **Step 1.5: Rewrite Example 2.5.1**

Find the example block starting `### Example 2.5.1` and replace its `cook` fence (the lines from ` ```cook` through ``` ``` ```):

```cook
recipe "build"
    ingredients "src/*.c" !"src/legacy.c"
    cook "bin/app" using "gcc {all} -o {out}"
end
```

The leading `CFLAGS "-Wall ..."` line is removed; the example now stands as a self-contained valid Cookfile that exercises three `STRING` roles (recipe name, ingredient pattern, `using` template).

- [ ] **Step 1.6: Drop test 11 from §2.10 line-classification cascade and renumber test 12 to 11**

In §2.10, delete the entire numbered item 11 (the `VarDecl` test):

```
11. Otherwise, if the trimmed line is `BARE_IDENTIFIER STRING` and the `BARE_IDENTIFIER` is not one of the blocking keywords of §{lexical.keywords}, the token is `VarDecl`. This test is applied both at column 0 and on indented lines; the syntactic layer rejects a `VarDecl` that appears after the first recipe (App. A.2).
```

Renumber the existing test 12 to test 11. The renumbered text:

```
11. Otherwise, the token is `Content`. Its further classification as `ingredients_step`, `cook_step`, `plate_step`, `test_step`, `module_call`, `interactive_command`, or `shell_command` is the concern of the step-dispatch cascade in App. A.4 and is made only inside a recipe body.
```

- [ ] **Step 1.7: Update the §2.10 ordering-significance and applicability paragraphs**

Find the paragraph that begins `The ordering is significant. In particular, test 3 ...` and replace `test 11 or 12` with `test 11` so the wording is:

```
The ordering is significant. In particular, test 3 (`>{`) MUST precede test 4 (`>`) so that a Lua-block prefix is not consumed as a Lua-line prefix followed by a literal `{` (§{lexical.line-prefixes}); tests 6–9 MUST require a separator so that identifiers whose first word merely starts with a keyword (for example, `recipes_cleanup`, `configure`, `useful`, `important`) fall through to test 11 and are not misclassified as declarations.
```

Find the paragraph immediately after that one (`Tests 1–9 and 11–12 apply ...`) and replace with:

```
Tests 1–9 and 11 apply to lines whether or not they are indented; only test 10 (implicit recipe header) is sensitive to column-0 positioning, by the constraint in App. A.3.
```

- [ ] **Step 1.8: Rewrite Example 2.10.1 to drop the `VarDecl` line**

Replace the `cook` fence inside `### Example 2.10.1` with:

```cook
use "cpp"                 # test 8:  UseDecl
config debug              # test 7:  ConfigHeader
    env.CC = "gcc"
end

build: lib setup          # test 10: implicit-header RecipeHeader
    # test 2: Comment
    ingredients "src/*.c" # test 11: Content (step-dispatched to ingredients_step)
    >{                    # test 3:  LuaBlockOpen
        print("hi")
    }
    recipes_cleanup       # test 11: Content (falls through test 6 — no separator)
end
```

The leading `CC "gcc"` (annotated `# test 11: VarDecl`) is removed; the `env.CC = CC` body line is replaced with a literal-string assignment that does not depend on a top-level VarDecl; the two `# test 12` comments become `# test 11` to match the renumbered cascade.

- [ ] **Step 1.9: Delete Note 2.10.1**

Find and delete the entire `### Note 2.10.1` heading and its paragraph (the one that begins `Test 11 admits VarDecl on indented lines ...`). After deletion, §2.10 ends with Example 2.10.1.

- [ ] **Step 1.10: Verify the build, vitest, and keyword lint all pass**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all three exit 0 with no errors. After Task 1 alone, §3.3 still defines the `grammar.var-declarations` slug (Task 2 deletes it), and no §2 reference to that slug survives — so the slug graph remains consistent and the build is green.

- [ ] **Step 1.11: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/02-lexical.mdx
git commit -m "$(cat <<'EOF'
spec(standard): remove VarDecl from §2 lexical

CS-0011 step 1/8. Drops the VarDecl token row from the §2.1 token
table, the variable-name supplementary-constraints row from §2.3,
the blocking-keyword half of §2.4 (and rewords the surviving
recipe-segment half), the variable_declaration mention in the §2.5
STRING-consumer inventory, the leading CFLAGS line in Example 2.5.1,
the test-11 line-classification rule and accompanying ordering /
applicability paragraphs in §2.10 (renumbering test 12 → test 11),
the CC line and "test 11/12" annotations in Example 2.10.1, and
Note 2.10.1 in its entirety. §3.3 still defines grammar.var-
declarations until Task 2 deletes it.
EOF
)"
```

---

## Task 2: Drop §3.3 and reconcile §3.1 / §3.2 / §3.8

**Files:**
- Modify: `standard/src/content/docs/03-syntactic-grammar.mdx`

This task removes the prose-grammar description of `variable_declaration` and reconciles every cross-reference to it inside chapter 3.

- [ ] **Step 2.1: Drop `variable_declaration` from the §3.1 toplevel-item list**

In §3.1 ("Grammar overview"), find the sentence:

```
A Cookfile is parsed as a single top-level production, `cookfile`, consisting of zero or more `toplevel_item`s (App. A.1). Each `toplevel_item` is a `recipe`, a `config_block`, a `use_declaration`, an `import_declaration`, a `variable_declaration`, a `comment`, or a `NEWLINE` (blank line).
```

Replace with:

```
A Cookfile is parsed as a single top-level production, `cookfile`, consisting of zero or more `toplevel_item`s (App. A.1). Each `toplevel_item` is a `recipe`, a `config_block`, a `use_declaration`, an `import_declaration`, a `comment`, or a `NEWLINE` (blank line).
```

- [ ] **Step 2.2: Update §3.1 Note 3.1.1 to drop the variable-declaration aside**

Find Note 3.1.1, which begins `The grammar is top-down and keyword-led at the outer layer ...`. Replace its body:

```
The grammar is top-down and keyword-led at the outer layer: the leading token of each `toplevel_item` is sufficient to commit to one of the six productions above, with no backtracking. The one form without a reserved keyword — `variable_declaration` — is distinguished at the lexical layer (§{lexical.identifiers}, §{lexical.keywords}): its leading token is a `BARE_IDENTIFIER` that is not a reserved keyword, immediately followed by a `STRING`.
```

with:

```
The grammar is top-down and keyword-led at the outer layer: the leading token of each `toplevel_item` is sufficient to commit to one of the five productions above, with no backtracking. Every top-level form is introduced by a reserved keyword (`recipe`, `config`, `use`, `import`) or is a `comment` / blank line; a top-level `Content` token (§{lexical.line-prefixes}) is not a valid `toplevel_item` and MUST be rejected.
```

- [ ] **Step 2.3: Update §3.2 ordering rule to drop `variable_declaration`**

Find the paragraph in §3.2 ("Top-level ordering") that begins `A variable_declaration, use_declaration, import_declaration, or config_block MUST NOT follow ...`. Replace:

```
A `variable_declaration`, `use_declaration`, `import_declaration`, or `config_block` MUST NOT follow the first `recipe` in a Cookfile. A conforming implementation MUST reject a Cookfile in which any such item appears after any `recipe`. The diagnostic MUST identify the offending item's source line.
```

with:

```
A `use_declaration`, `import_declaration`, or `config_block` MUST NOT follow the first `recipe` in a Cookfile. A conforming implementation MUST reject a Cookfile in which any such item appears after any `recipe`. The diagnostic MUST identify the offending item's source line.
```

Find the next paragraph (`Within each ordered prefix, variable_declarations, use_declarations, import_declarations, and config_blocks MAY appear ...`) and replace with:

```
Within each ordered prefix, `use_declaration`s, `import_declaration`s, and `config_block`s MAY appear in any order and MAY be interleaved freely.
```

- [ ] **Step 2.4: Rewrite Example 3.2.1**

Replace the `cook` fence inside `### Example 3.2.1` with:

```cook
use cpp
import backend ./services/backend

config
    env.CC = "gcc"
end

config release
    env.CXXFLAGS = "-O3"
end

recipe build
    gcc -o main main.c
end
```

Then replace the explanatory paragraph immediately below the fence:

```
Each of the four ordered forms appears before the single `recipe`. The `use`, `import`, variable, and `config` items are in the prefix; any of them placed after `recipe build` would be rejected.
```

with:

```
Each of the three ordered forms (`use_declaration`, `import_declaration`, `config_block`) appears before the single `recipe`. The `use`, `import`, and `config` items are in the prefix; any of them placed after `recipe build` would be rejected.
```

- [ ] **Step 2.5: Update Note 3.2.1 to drop the variable-declaration test reference**

Replace Note 3.2.1's body:

```
The `cook-lang` parser test `test_parse_use_after_recipe_fails` (and companion tests for `import` and `config`) pin this ordering rule.
```

with:

```
The `cook-lang` parser test `test_parse_use_after_recipe_fails` (and companion tests for `import` and `config`) pin this ordering rule.
```

(The note text is already correct — no edit required if you grep and confirm it does not list a `variable_declaration` companion. If the body matches the form above unchanged, advance to the next step without modification.)

- [ ] **Step 2.6: Delete §3.3 ("Variable declarations") in its entirety**

Delete every line from the heading `## 3.3. Variable declarations [#grammar.var-declarations]` through the end of Note 3.3.1 inclusive. After deletion, §3.2 is followed directly by `## 3.4. \`use\` declarations [#grammar.use-declarations]` — do not renumber §3.4 onward; the slug-based ref system makes the section number a heading-time concern only.

(Section numbering in headings: keep the existing `## 3.4.`, `## 3.5.`, `## 3.6.`, `## 3.7.`, `## 3.8.` markers untouched. Skipping §3.3 in the rendered numbering is preferred over renumbering, because the slugs decouple identity from order; renumbering would create a churn of inbound `§{...}` references that resolve to different display numbers.)

Actually, re-evaluate: the slug refs render as `current section number`, so a renumber DOES change the displayed numerals at every call site. But because we keep all slugs stable, the *links* still resolve and the *prose* identities are preserved. Whether the displayed numbers shift is purely cosmetic. To minimize visible churn, keep the existing `## 3.4. ... ## 3.8.` markers and let §3.3 simply become a gap. This matches the pattern already used elsewhere in the Standard after CS-0009 restructured §5/§7/§8 (see commit `f636635 spec(standard): update TOC for § 5/7/8 renumber (CS-0009)` — the renumber was done; gaps are uncommon but acceptable).

Decision: keep §3.4-§3.8 numbering. §3.3 becomes a gap. No renumber sweep needed.

- [ ] **Step 2.7: Delete the §3.8 paragraph that reclassifies `VarDecl` inside a recipe body**

Find the paragraph in §3.8 ("Step dispatch inside a recipe") that begins `A VarDecl token that appears inside a recipe_body MUST NOT produce a variable_declaration step.` Delete the entire paragraph:

```
A `VarDecl` token that appears inside a `recipe_body` MUST NOT produce a `variable_declaration` step. A conforming implementation MUST reconstitute the original source text (`NAME "value"`) and dispatch it as a `shell_command` step. Variable declarations are a top-level-only construct (§{grammar.var-declarations}).
```

- [ ] **Step 2.8: Update §3.8 to drop `VarDecl` from the lexical-classification list**

Find the paragraph in §3.8 that begins `Inside a recipe_body, each source line is first classified by the lexer ...`. Replace:

```
Inside a `recipe_body`, each source line is first classified by the lexer (§{lexical.line-classification}) into one of `Comment`, `Blank`, `LuaLine`, `LuaBlockOpen`, `VarDecl`, or `Content`. Comments and blank lines are skipped; `LuaLine` becomes a `lua_line` step; `LuaBlockOpen` begins a `lua_block` step collected by the brace-balance algorithm (§{lexical.brace-blocks}).
```

with:

```
Inside a `recipe_body`, each source line is first classified by the lexer (§{lexical.line-classification}) into one of `Comment`, `Blank`, `LuaLine`, `LuaBlockOpen`, or `Content`. Comments and blank lines are skipped; `LuaLine` becomes a `lua_line` step; `LuaBlockOpen` begins a `lua_block` step collected by the brace-balance algorithm (§{lexical.brace-blocks}).
```

- [ ] **Step 2.9: Update the §3.8 example explanation to drop the VarDecl callout**

Find the paragraph immediately after Example 3.8.1 that begins `The body line CC "gcc" would be a variable_declaration at the top level ...`. Replace:

```
The body line `CC "gcc"` would be a `variable_declaration` at the top level but is dispatched as a `shell_command` inside a recipe body. The emitted command preserves the original surface form — the identifier and its quoted argument.
```

with:

```
The body line `CC "gcc"` is a `Content` token; the step-dispatch cascade matches no preceding alternative and dispatches it as a `shell_command` whose text is the trimmed source line. The emitted command preserves the original surface form — the identifier and its quoted argument.
```

- [ ] **Step 2.10: Run the build, vitest, and keyword lint**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all three exit 0. After Task 2, no `§{...}` reference targets `grammar.var-declarations` (the slug definition itself is gone with the §3.3 deletion), and no prose references the deleted `VarDecl` token.

If `pnpm build` reports a broken slug ref to `grammar.var-declarations`, grep for the slug across all `standard/src/content/docs/` files to find the missed reference; the only legitimate consumers were the rows and paragraphs already edited above.

- [ ] **Step 2.11: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/03-syntactic-grammar.mdx
git commit -m "$(cat <<'EOF'
spec(standard): remove VarDecl from §3 syntactic grammar

CS-0011 step 2/8. Drops §3.3 in its entirety, the variable_declaration
entry from §3.1's toplevel_item list, Note 3.1.1's keyword-vs-non-
keyword aside, the §3.2 ordering reference, the variable mention in
Example 3.2.1's explanation, the §3.8 reclassification paragraph, and
the §3.8 example's VarDecl callout. Re-bases Example 3.2.1 on a config
block. The §3.6 forward reference to §3.6.1 (config-block composition)
lands together with §3.6.1 itself in Task 5.
EOF
)"
```

---

## Task 3: Drop Note 4.4.2 from §4

**Files:**
- Modify: `standard/src/content/docs/04-recipes.mdx`

- [ ] **Step 3.1: Delete Note 4.4.2 in its entirety**

Find the heading `### Note 4.4.2` (around line 134) and the paragraph that follows it. Delete both the heading and the paragraph:

```
### Note 4.4.2

A top-level `variable_declaration` — a line of the form `NAME "value"` — is lexed as a `Token::VarDecl` regardless of position in the file. Inside a recipe body, a conforming implementation MUST reclassify a `VarDecl` token as a `shell_command` whose text is the original source (§{grammar-appendix.steps}, `shell_command` grammar comment). See parser test `test_var_after_recipe_is_shell_command`.
```

After deletion, Note 4.4.1 is followed directly by `## 4.5. \`cook\` step — single-output form [#recipes.cook-single-output]`.

- [ ] **Step 3.2: Run the verification trio**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all green. The build's `rehype-bare-ref-lint` confirms no slug ref to `grammar.var-declarations` survives in any file.

- [ ] **Step 3.3: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/04-recipes.mdx
git commit -m "$(cat <<'EOF'
spec(standard): drop §4 Note 4.4.2 (VarDecl reclassification)

CS-0011 step 3/8. The note's claim — that a top-level VarDecl line
inside a recipe body is reclassified as a shell_command — is moot
after §3.3 deletion: the line is now a Content token from the start
and the step-dispatch cascade dispatches it as a shell_command via
priority 7, with no special-case wording required.
EOF
)"
```

---

## Task 4: Update Appendix A formal grammar

**Files:**
- Modify: `standard/src/content/docs/appendix/A-grammar.mdx`

This task brings the formal EBNF and its accompanying notes into agreement with §2 and §3.

- [ ] **Step 4.1: Drop `variable_declaration` from A.1 `toplevel_item` alternation**

In the §A.1 fenced EBNF block, find the lines:

```ebnf
toplevel_item         ::= recipe
                       | config_block
                       | use_declaration
                       | import_declaration
                       | variable_declaration
                       | comment
                       | NEWLINE
```

Replace with:

```ebnf
toplevel_item         ::= recipe
                       | config_block
                       | use_declaration
                       | import_declaration
                       | comment
                       | NEWLINE
```

- [ ] **Step 4.2: Drop the `variable_declaration` production from §A.2**

In the §A.2 fenced EBNF block, find the line:

```ebnf
variable_declaration  ::= BARE_IDENTIFIER STRING NEWLINE
```

Delete it. The block then begins with `use_declaration`.

- [ ] **Step 4.3: Update the §A.2 ordering normative paragraph**

Find the paragraph in §A.2:

```
**Ordering (normative).** `variable_declaration`, `use_declaration`, `import_declaration`, and `config_block` MUST appear before the first `recipe`. A conforming implementation MUST reject a Cookfile that places any of these after a recipe.
```

Replace with:

```
**Ordering (normative).** `use_declaration`, `import_declaration`, and `config_block` MUST appear before the first `recipe`. A conforming implementation MUST reject a Cookfile that places any of these after a recipe.
```

- [ ] **Step 4.4: Delete the §A.2 "Identifiers blocked from `variable_declaration`" rule**

Delete the entire paragraph:

```
**Identifiers blocked from `variable_declaration`.** The identifiers `recipe`, `config`, `end`, `ingredients`, `cook`, `plate`, `using`, `use`, `import`, `test` MUST NOT be used as the left-hand side of a `variable_declaration`. A conforming implementation MUST reject any such declaration.
```

- [ ] **Step 4.5: Update the §A.4 `shell_command` grammar comment**

In the §A.4 EBNF block, find the `shell_command` production with its trailing comment:

```ebnf
shell_command         ::= SHELL_LINE_CONTENT NEWLINE
                          /* Any Content line not matching a preceding
                             alternative. A line of the form NAME "value"
                             (which would be a variable_declaration at the
                             top level) is treated as a shell_command inside
                             a recipe body. See § 3.8. */
```

Replace with:

```ebnf
shell_command         ::= SHELL_LINE_CONTENT NEWLINE
                          /* Any Content line not matching a preceding
                             alternative. See § 3.8. */
```

- [ ] **Step 4.6: Update the §A.5 "BARE_IDENTIFIER vs name" note**

Find the paragraph in §A.5:

```
**`BARE_IDENTIFIER` vs `name`.** Recipe and dependency names can be either a `BARE_IDENTIFIER` (allowing dots and hyphens as internal characters) or a double-quoted `STRING`. The `import` declaration's local alias is always a `BARE_IDENTIFIER` (no quoting). Variable declaration left-hand sides are always `BARE_IDENTIFIER`.
```

Replace with:

```
**`BARE_IDENTIFIER` vs `name`.** Recipe and dependency names can be either a `BARE_IDENTIFIER` (allowing dots and hyphens as internal characters) or a double-quoted `STRING`. The `import` declaration's local alias is always a `BARE_IDENTIFIER` (no quoting).
```

- [ ] **Step 4.7: Run the verification trio**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all green.

- [ ] **Step 4.8: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/appendix/A-grammar.mdx
git commit -m "$(cat <<'EOF'
spec(standard): drop variable_declaration from Appendix A

CS-0011 step 4/8. Removes variable_declaration from the App. A.1
toplevel_item alternation, the App. A.2 production, the App. A.2
ordering rule, the "Identifiers blocked" rule, the App. A.4
shell_command grammar comment's parenthetical, and the App. A.5
"BARE_IDENTIFIER vs name" note's last sentence.
EOF
)"
```

---

## Task 5: Add §3.6.1 normative composition section

**Files:**
- Modify: `standard/src/content/docs/03-syntactic-grammar.mdx`

This task inserts the new normative composition rules and example.

- [ ] **Step 5.1: Add the §3.6 lead-in pointing to the new §3.6.1**

In `standard/src/content/docs/03-syntactic-grammar.mdx`, find the §3.6 heading and the four paragraphs that follow (through the closing of the §3.6 normative prose, immediately before `### Example 3.6.1`). At the end of the §3.6 prose — immediately before `### Example 3.6.1` — insert:

```
Composition of multiple `config_block`s — base + at most one selected overlay — is specified in §{grammar.config-composition}.
```

This forward reference resolves to the new §3.6.1 inserted in step 5.2 below; both edits are part of the same commit, so the slug exists by the time the build runs.

- [ ] **Step 5.2: Insert the §3.6.1 heading and normative paragraphs**

In the same file, find the `### Note 3.6.1` heading and its body paragraph (the "config_body is collected by source-line range ..." note). Immediately AFTER that note (and before `## 3.7. Recipes [#grammar.recipe-syntax]`), insert:

```mdx
### 3.6.1. Config-block composition [#grammar.config-composition]

A Cookfile MAY contain at most one unnamed `config_block` (the *base config*) and zero or more named `config_block`s (the *overlay configs*). At load time, a conforming implementation MUST select at most one overlay config by name. The selection mechanism is implementation-defined.

A conforming implementation MUST report a load-time error when the selected name does not match any named `config_block` declared in the Cookfile. The diagnostic MUST identify the requested name.

When a base config is present, its body MUST execute first against the Cook Lua API state (§{lua}). When an overlay is selected, its body MUST execute second against the same state. The overlay's writes therefore observe values established by the base; an overlay write to a key already set by the base replaces the base's value (last-write-wins). When no overlay is selected, only the base (if present) executes. When neither is present, no `config_block` Lua executes during the load phase.

Both bodies execute during the load phase, after `use` resolution (§{lua.use-env}) and before recipe registration (§{exec}).

#### Example 3.6.1.1

```cook
use cpp

config
    env.CC = "gcc"
    env.CXXFLAGS = "-O0 -g"
end

config release
    env.CXXFLAGS = "-O3"
end

config dev
    env.CXXFLAGS = "-O0 -g -DDEBUG"
end

recipe build
    cook "build/main" using "{CC} {CXXFLAGS} -o {out} main.c"
end
```

When no overlay is selected, the base alone executes and `{CXXFLAGS}` resolves to `-O0 -g`. When `release` is selected, the base runs first (`env.CXXFLAGS = "-O0 -g"`), then `release` runs (`env.CXXFLAGS = "-O3"`); `{CXXFLAGS}` resolves to `-O3`. Selecting a name that does not appear as a named `config_block` in the Cookfile is a load-time error.

#### Note 3.6.1.1

Multi-overlay selection is deliberately not specified in this revision: a conforming implementation MUST permit at most one overlay. The single-overlay restriction avoids ordering questions across multiple selected overlays and is non-breaking to relax in a future revision; see App. B.3.9.
```

- [ ] **Step 5.3: Run the verification trio**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all green. The new slug `grammar.config-composition` is now defined, satisfying the forward reference added in step 5.1.

The `lint:keywords` check is the most likely to flag something: the new prose uses many uppercase RFC-2119 keywords (`MUST`, `MAY`). Sanity-scan your inserted text for any lowercase `must`/`shall`/`should`/`may` outside the fenced code block — there should be none.

The forward reference to `App. B.3.9` will resolve in Task 7, but the lint and build pass now because that reference is plain prose ("App. B.3.9") and not a `§{...}` slug ref.

- [ ] **Step 5.4: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/03-syntactic-grammar.mdx
git commit -m "$(cat <<'EOF'
spec(standard): add §3.6.1 config-block composition

CS-0011 step 5/8. Specifies normative composition for config_blocks:
at most one unnamed (base) plus at most one named (overlay) selected
at load time by an implementation-defined mechanism. Base executes
first, overlay second, against the same Cook Lua API state, with
last-write-wins on overlay writes. Both run after use resolution and
before recipe registration. Adds Example 3.6.1.1 showing release/dev
overlays over a CC/CXXFLAGS base, and Note 3.6.1.1 reserving the
single-overlay restriction as non-breaking-to-relax.
EOF
)"
```

---

## Task 6: Add §6 normative `env` alias paragraph

**Files:**
- Modify: `standard/src/content/docs/06-cook-lua-api.mdx`

- [ ] **Step 6.1: Identify the insertion point**

Read the §6.1 section heading and the introductory paragraph(s). The new `env`-alias paragraph belongs at the end of the chapter intro (before §6.2 begins), so it lives where readers encountering the Cook Lua API for the first time meet it.

- [ ] **Step 6.2: Insert the alias paragraph**

Add a new heading + paragraph block immediately before the next-section heading following §6.1's intro. The exact text:

```mdx
### 6.1.x. The `env` alias inside `config_block` bodies [#lua.env-alias]

Within the body of a `config_block` (§{grammar.config-blocks}), the bare global `env` MUST be bound such that `env` and `cook.env` refer to the same table. Writes through either name MUST be observable through both. The `env` alias is in scope only within `config_block` bodies; it MUST NOT be bound in recipe-body Lua, in `using` blocks, in module bodies, or at any other Lua entry point.
```

If §6.1 already has numbered subsections (`### 6.1.1`, `### 6.1.2`, etc.), pick the next free index and substitute it for the placeholder `x` (for example, `### 6.1.4` if §6.1.3 is the highest existing one). If §6.1 has no subsections, use `### 6.1.1`. Read the file first; do not insert without confirming the actual numbering.

- [ ] **Step 6.3: Run the verification trio**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all green. The new slug `lua.env-alias` is registered. The `MUST` and `MUST NOT` uses are uppercase.

- [ ] **Step 6.4: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/06-cook-lua-api.mdx
git commit -m "$(cat <<'EOF'
spec(standard): bind env as cook.env alias in config bodies (§6)

CS-0011 step 6/8. Defines `env` as a normative alias for `cook.env`
inside config_block bodies only. Writes through either name are
observable through both. The alias is not bound in recipe bodies,
using blocks, module bodies, or other Lua entry points. This brings
the env.X = "value" usage already shown in §3.2.1 / §3.6.1 / §3.6.1.1
examples into normative conformance.
EOF
)"
```

---

## Task 7: Rationale (Appendix B) updates

**Files:**
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx`

This task rewrites B.2.4 to drop the variable-declaration half of the contextual-keyword justification, deletes B.3.8 (whose entire subject is a moot reclassification), and adds a new B.3.9 capturing the design rationale for collapsing variables into config blocks.

- [ ] **Step 7.1: Rewrite B.2.4 to motivate contextual reservation purely on recipe segments**

Find `### B.2.4. Why keywords are reserved contextually, not globally [#rationale.contextual-keywords]` and the three paragraphs that follow it. Replace the entire subsection (heading + three paragraphs) with:

```mdx
### B.2.4. Why keywords are reserved contextually, not globally [#rationale.contextual-keywords]
The Cookfile language has reserved recipe segments (`stem`, `name`, `ext`, `dir`, `in`, `out`, `all`), and the set is not reserved globally. A recipe named `backend.build` is accepted because `build` is not on the reserved-segment list; `backend.stem` is rejected because `stem` is.

Global reservation would have been simpler to specify but would have broken the most common shell idioms. Cookfiles frequently call tools named `configure`, `cook-book`, `tester`, `using-*` — forbidding those as shell commands to protect a handful of language keywords would be a poor trade. The contextual rule instead places the reservation only where it is needed: the final segment of a recipe name, where the `{stem}`-style substitution rule would otherwise collide with a literal segment of the same name.

The cost is that the step-dispatch cascade in App. A.4 must insist on a separator after each step keyword (`recipe`, `config`, `cook`, `plate`, `ingredients`, `using`, `use`, `import`, `end`, `test`) so that a `Content` line whose first word is `recipes_cleanup` or `configure` is not misclassified as a declaration. That cost is borne by the Standard and the implementation, not by the author.
```

- [ ] **Step 7.2: Delete B.3.8 in its entirety**

Find `### B.3.8. Why \`NAME "value"\` inside a recipe body is a shell command [#rationale.name-value-shell]` and delete the heading and all three paragraphs that follow it (through the line that begins `The rule is symmetric with the top-level-only classification of use, import, and config ...`). After deletion, the previous B.3 subsection (B.3.7) is followed directly by `## B.4. On §{recipes} Recipes and step kinds [#rationale.recipes]`.

- [ ] **Step 7.3: Insert new B.3.9 ("Config blocks as the sole variable surface")**

Immediately before the `## B.4.` heading, insert a new subsection:

```mdx
### B.3.9. Config blocks as the sole variable surface [#rationale.config-only-variables]
A previous revision of the Standard admitted a top-level `variable_declaration` form (`NAME "value"`) that fed values into `cook.env` independently of any `config_block`. CS-0011 removes that form. The remaining surface is the `config_block` Lua body, which writes `cook.env` (and any other Cook API state) through ordinary Lua assignment.

Three forces motivate the consolidation. First, named overlay configs cannot override values set outside any config block without ad-hoc precedence wording; folding the surface into config blocks gives a single layered model with well-defined override semantics (§{grammar.config-composition}). Second, the bare top-level form is sugar for one specific case of what config block bodies already express; removing it deletes a duplicate surface, the `Token::VarDecl` lexical class, the contextual blocking-keyword reservation, and the recipe-body reclassification rule that the form required. Third, the consolidation moves the choice of *when* a value is set (always, vs. only-when-an-overlay-is-active) into the same surface as the value itself, which keeps the source of truth localised.

Layered single-select was chosen over multi-select for this revision: at most one overlay can be active at a time. Multi-select would force the Standard to specify ordering across multiple selected overlays and to define conflict resolution for keys written by more than one of them. Single-select avoids both questions and is non-breaking to relax in a future revision if real use cases warrant it.

The selection mechanism for the active overlay is implementation-defined. Config selection is a CLI affordance, and the Standard's general posture (§{intro}) is to specify Cookfile-language behaviour rather than tool invocation surfaces. Implementations are free to expose a flag, an environment variable, a file-level default, or any combination, provided the implementation rejects a selection that does not match a declared overlay.
```

- [ ] **Step 7.4: Run the verification trio**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all green. The forward reference to `App. B.3.9` from Task 5 step 5.1 (Note 3.6.1.1) now has a target. The new slug `rationale.config-only-variables` resolves cleanly. The deleted B.3.8 had no inbound `§{...}` references (verified during design); the build's `rehype-bare-ref-lint` confirms.

If `lint:keywords` flags lowercase `should` or similar in your new prose, scan and capitalize. The B.3.9 text above uses uppercase `MUST` only inside the implementation-rejection clause, which is correct for an informative annex citing a normative rule from §3.6.1.

- [ ] **Step 7.5: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/appendix/B-rationale.mdx
git commit -m "$(cat <<'EOF'
spec(standard): rationale (App. B) updates for CS-0011

CS-0011 step 7/8. Rewrites B.2.4 to motivate contextual keyword
reservation purely on the reserved-recipe-segment set (the variable-
declaration half is gone). Deletes B.3.8, whose entire subject — the
recipe-body reclassification of NAME "value" — is moot once VarDecl
is removed; the line is now an ordinary Content token dispatched by
priority 7. Adds new B.3.9 capturing the design rationale for
consolidating variables into config blocks: composition motivation,
single-mechanism win, single-overlay choice, implementation-defined
selection.
EOF
)"
```

---

## Task 8: Add CS-0011 D-changes entry

**Files:**
- Modify: `standard/src/content/docs/appendix/D-changes.mdx`

- [ ] **Step 8.1: Append the CS-0011 entry to Appendix D**

In `standard/src/content/docs/appendix/D-changes.mdx`, append a new entry at the end of the file (after the existing CS-0010 entry). Use the heading format that CS-0010 uses (`## D.NN. CS-NNNN — title. [#changes.cs-NNNN]`):

```mdx
## D.11. CS-0011 — Remove top-level variable declarations. [#changes.cs-0011]

**Date:** 2026-04-26
**Sections affected:** §{lexical.tokens}, §{lexical.identifiers}, §{lexical.keywords}, §{lexical.strings}, §{lexical.line-classification} (test-cascade renumber); §{grammar.overview}, §{grammar.top-level-ordering}, §{grammar.config-blocks}, §{grammar.step-dispatch}; §{grammar.var-declarations} (deleted); §{grammar.config-composition} (new); §{recipes.step-kinds} (Note 4.4.2 deleted); §{lua.env-alias} (new); App. A.1, A.2, A.4, A.5; App. B.2.4 (rewritten), B.3.8 (deleted), B.3.9 (new).

**Summary:** Removes the top-level `variable_declaration` form (`NAME "value"`) and its supporting machinery: the `VarDecl` lexer token (§{lexical.tokens}), the blocking-keyword half of the contextual reservation (§{lexical.keywords}), the line-classification cascade test for the form (§{lexical.line-classification} test 11; the former test 12 renumbers to 11), and the recipe-body reclassification rule (§{grammar.step-dispatch}). The §3.3 prose-grammar section, the App. A.2 production, the App. A.2 "Identifiers blocked" rule, and the App. A.4 `shell_command` grammar comment's parenthetical reference are all deleted; App. B.2.4 is rewritten to motivate the contextual-keyword rule purely on the reserved-recipe-segment set, and App. B.3.8 is deleted as moot.

In place of the removed surface, the Standard adds §{grammar.config-composition} (new §3.6.1) specifying that a Cookfile MAY contain at most one unnamed `config_block` (the *base config*) and zero or more named `config_block`s (the *overlay configs*); a conforming implementation MUST select at most one overlay at load time by an implementation-defined mechanism, MUST reject a selection that does not match any declared overlay, and MUST execute base before overlay against shared Cook Lua API state with last-write-wins semantics on overlay writes. Both bodies execute during the load phase, after `use` resolution and before recipe registration. §{lua.env-alias} (new §6.1.x) binds the bare global `env` to `cook.env` inside `config_block` bodies only, bringing the `env.X = "..."` usage already shown in examples into normative conformance. App. B.3.9 (new) records the design rationale for consolidating variables into config blocks, the single-overlay choice over multi-select, and the implementation-defined selection mechanism.

**Implementation status.** This change is spec-only. The Rust parser (`cli/crates/cook-lang`) and `tree-sitter-cook` continue to accept the old form; bringing them into conformance with the Standard at this version is the subject of a follow-up CS that will also add the §3.6.1 conformance fixtures (base-only, base + overlay last-write-wins, overlay-only, missing-overlay-is-error, base-runs-before-overlay).

**Reference:** this commit.
```

- [ ] **Step 8.2: Run the verification trio**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all green. The new CS-0011 entry's slug `changes.cs-0011` is harvested by `cs-ids.ts`; the build emits the permalink for the entry. The slug references inside the entry's "Sections affected" list all resolve to existing or newly-created slugs.

- [ ] **Step 8.3: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/appendix/D-changes.mdx
git commit -m "$(cat <<'EOF'
spec(standard): add CS-0011 D-changes entry for VarDecl removal

CS-0011 step 8/8. Records the spec-only change in App. D: removal of
the VarDecl token, the variable_declaration production, the blocking-
keyword reservation, and the recipe-body reclassification rule;
addition of the §3.6.1 config-block composition rules and the §6.1.x
env-alias binding; rationale rewrites in App. B.2.4 / B.3.8 / B.3.9.
Notes that the parser and tree-sitter follow-up is the subject of a
later CS that will also add the §3.6.1 conformance fixtures.
EOF
)"
```

---

## Task 9: Final verification and Rust conformance harness sanity check

**Files:** none modified.

This task is the all-up sanity pass. After Tasks 1–8 the Standard is internally coherent; this task confirms the broader project still works.

- [ ] **Step 9.1: Confirm the Astro build, tests, and keyword lint are still green**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all three exit 0. No new findings since Task 8.

- [ ] **Step 9.2: Confirm the Rust conformance harness still passes against the existing fixtures**

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang --test conformance
```

Expected: all conformance tests pass. The harness compares the parser's AST output against `standard/conformance/positive/*/parse.txt` and the rejection behaviour against `standard/conformance/negative/*/`. None of the existing fixtures uses a top-level `NAME "value"` form (verified during design — `find standard/conformance -name Cookfile -exec grep -l ... {} \;` returned no matches). The parser still emits a `vars: []` line in its AST output for any Cookfile, which is what every fixture's parse.txt expects, so no fixture comparison observes the implementation-vs-spec divergence introduced by this CS.

If the harness fails, do NOT modify fixtures or parser code in this plan — the failure indicates a fixture this plan did not anticipate. Stop, report the failure, and treat it as scope-creep that belongs in the parser follow-up CS.

- [ ] **Step 9.3: Confirm the spec-first pre-commit hook is satisfied across the full series**

```bash
cd /home/alex/dev/cook
git log --oneline -8
```

Expected: eight new commits on `main`, each with a `spec(standard): ...` subject line, in the order Task 1 → Task 2 → Task 3 → Task 4 → Task 5 → Task 6 → Task 7 → Task 8. The hook gates only commits that touch language-surface paths without a corresponding `standard/` change — every commit in this series touches only `standard/`, so the hook should have permitted each one.

- [ ] **Step 9.4: Read through the rendered chapters as a final sanity check**

```bash
cd /home/alex/dev/cook/standard
pnpm dev
```

Open the printed local URL in a browser. Walk through:

- §2.4 — confirm the keywords section reads as a coherent single-table description with no dangling reference to "blocking keywords" or "variable declarations."
- §3.1 — confirm the toplevel_item list has five entries.
- §3.2 — confirm the example uses a `config` block, not a `CC "gcc"` line.
- §3.3 — confirm it is gone (the rendered TOC shows §3.2 followed by §3.4).
- §3.6 — confirm the new §3.6.1 ("Config-block composition") subsection is present and the example is well-formed.
- §3.8 — confirm the body-classification list and the example explanation no longer mention `VarDecl`.
- §4.4 — confirm Note 4.4.2 is gone.
- §6 — confirm the new env-alias subsection is present with normative MUST / MUST NOT.
- App. A — confirm `variable_declaration` is gone from A.1 and A.2 EBNF blocks.
- App. B.2.4 — confirm the rewrite removes the variable-declaration half.
- App. B.3 — confirm B.3.8 is gone and B.3.9 ("Config blocks as the sole variable surface") is present.
- App. D — confirm CS-0011 is the bottom entry.

Stop the dev server (Ctrl-C) when done.

- [ ] **Step 9.5: No commit for this task**

This task is verification-only; no files changed.

---

## Self-review checklist (run before handing off to executing-plans)

Following the writing-plans skill's self-review protocol. Each item below is a quick gut-check against the design spec.

**1. Spec coverage:** Every section of `standard/specs/2026-04-26-remove-vardecl-design.md` maps to a task above:
- Design §3.1 (lexical) → Task 1
- Design §3.2 (syntactic + Appendix A) → Tasks 2, 4
- Design §3.3 (new composition) → Task 5
- Design §3.4 (env alias) → Task 6
- Design §3.5 (cross-recipe references unchanged) → no task needed (verified by Task 9 build pass)
- Design §3.6 (rationale) → Task 7
- Design §3.7 (D-changes + conformance) → Task 8 (D-changes); conformance fixtures explicitly out-of-scope per design §2 / Task 8 step 8.1's "Implementation status" paragraph
- Design §4 (out-of-scope items) → no task; explicitly deferred
- Design §5 (review checklist for the implementation plan) → satisfied by Task 9

**2. Placeholder scan:** No "TBD" / "TODO" / "implement later" / "fill in details" anywhere. The single `x` placeholder in Task 6 step 6.2 (`### 6.1.x.`) has explicit instructions for the implementer to read the file and pick the next free numeric index — it is bounded and actionable, not hand-wavy.

**3. Type / name consistency:** Slug names used across tasks are consistent: `grammar.config-composition` (new in Task 5, referenced from Task 2 step 2.7 and Task 8 step 8.1); `lua.env-alias` (new in Task 6, referenced from Task 8); `rationale.config-only-variables` (new in Task 7, referenced from Task 5 step 5.1 as "App. B.3.9" plain text and from Task 8 step 8.1). CS ID is consistent: CS-0011 throughout.

**4. Section-numbering consistency:** §3.3 becomes a gap (Task 2 step 2.6); §3.4–§3.8 retain their numbers. §3.6.1 / §6.1.x are new subsections. The new App. B subsection is B.3.9 (next after the existing B.3.7; B.3.8 is deleted). App. D entry is CS-0011 with display heading `D.11.`.

---

## Execution Handoff

**Plan complete and saved to `standard/plans/2026-04-26-remove-vardecl-plan.md`. Two execution options:**

**1. Subagent-Driven (recommended)** - Dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
