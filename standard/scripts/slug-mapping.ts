// Slug mapping for the Cook Standard.
//
// Keyed by the pre-migration positional anchor ID (sec-N-M-K). The migration
// script in this directory reads this map to rewrite headings and refs.
// After migration, this file remains in-tree as the authoritative registry
// of chapter prefixes and their clauses — future renames update it alongside
// the heading markers.

export const SLUG_MAPPING: Record<string, string> = {
  // ── Chapter 0 — Introduction ──────────────────────────────────────────────
  'sec-0':     'intro',
  'sec-0-1':   'intro.purpose',
  'sec-0-2':   'intro.scope',
  'sec-0-3':   'intro.non-scope',
  'sec-0-4':   'intro.normative-informative',
  'sec-0-5':   'intro.version',
  'sec-0-6':   'intro.architecture-relationship',
  'sec-0-7':   'intro.conformance',

  // ── Chapter 1 — Notation and conventions ──────────────────────────────────
  'sec-1':     'notation',
  'sec-1-1':   'notation.keywords',
  'sec-1-2':   'notation.numbering-and-citation',
  'sec-1-3':   'notation.normative-blocks',
  'sec-1-4':   'notation.grammar',
  'sec-1-5':   'notation.grammar-precedence',
  'sec-1-6':   'notation.amendment-markers',
  'sec-1-7':   'notation.stable-anchors',

  // ── Chapter 2 — Lexical structure ─────────────────────────────────────────
  'sec-2':     'lexical',
  'sec-2-1':   'lexical.source-representation',
  'sec-2-2':   'lexical.tokens',
  'sec-2-3':   'lexical.identifiers',
  'sec-2-4':   'lexical.keywords',
  'sec-2-5':   'lexical.strings',
  'sec-2-6':   'lexical.comments',
  'sec-2-7':   'lexical.line-prefixes',
  'sec-2-8':   'lexical.numbers',
  'sec-2-9':   'lexical.brace-blocks',
  'sec-2-10':  'lexical.line-classification',

  // ── Chapter 3 — Syntactic grammar ─────────────────────────────────────────
  'sec-3':     'grammar',
  'sec-3-1':   'grammar.overview',
  'sec-3-2':   'grammar.top-level-ordering',
  'sec-3-3':   'grammar.var-declarations',
  'sec-3-4':   'grammar.use-declarations',
  'sec-3-5':   'grammar.import-declarations',
  'sec-3-6':   'grammar.config-blocks',
  'sec-3-7':   'grammar.recipe-syntax',
  'sec-3-8':   'grammar.step-dispatch',

  // ── Chapter 4 — Recipes and step kinds ────────────────────────────────────
  'sec-4':     'recipes',
  'sec-4-1':   'recipes.header-forms',
  'sec-4-1-1': 'recipes.termination',
  'sec-4-2':   'recipes.dep-list',
  'sec-4-3':   'recipes.ingredients',
  'sec-4-4':   'recipes.step-kinds',
  'sec-4-5':   'recipes.cook-single-output',
  'sec-4-6':   'recipes.cook-multi-output',
  'sec-4-7':   'recipes.plate-step',
  'sec-4-8':   'recipes.test-step',
  'sec-4-9':   'recipes.lua-steps',
  'sec-4-10':  'recipes.shell-steps',
  'sec-4-11':  'recipes.module-call-steps',

  // ── Chapter 4a — Chores ───────────────────────────────────────────────────
  'sec-4a':       'chores',
  'sec-4a-1':     'chores.header',
  'sec-4a-2':     'chores.body',
  'sec-4a-3':     'chores.default-interactive',
  'sec-4a-4':     'chores.no-caching',
  'sec-4a-5':     'chores.cross-form-deps',

  // ── Chapter 5 — Cross-recipe references ───────────────────────────────────
  'sec-5':       'xref',
  'sec-5-1':     'xref.name-references',
  'sec-5-2':     'xref.resolution',
  'sec-5-2-1':   'xref.dotted-names',
  'sec-5-2-2':   'xref.reserved-segment',
  'sec-5-2-3':   'xref.env-shadowing',
  'sec-5-3':     'xref.path-accessors',
  'sec-5-4':     'xref.dep-driven',
  'sec-5-4-1':   'xref.dep-recipe-output',
  'sec-5-4-2':   'xref.dep-driven-example',
  'sec-5-5':     'xref.string-substitution',
  'sec-5-5-1':   'xref.string-substitution-example',
  'sec-5-6':     'xref.dep-implications',

  // ── Chapter 6 — Cook Lua API ───────────────────────────────────────────────
  'sec-6':       'lua',
  'sec-6-1':     'lua.api-overview',
  'sec-6-1-1':   'lua.recipe-global',
  'sec-6-2':     'lua.add-unit',
  'sec-6-3':     'lua.step-helpers',
  'sec-6-3-1':   'lua.cook-sh',
  'sec-6-3-2':   'lua.cook-exec',
  'sec-6-3-3':   'lua.cook-recipe',
  'sec-6-4':     'lua.using-block-globals',
  'sec-6-5':     'lua.fs-helpers',
  'sec-6-5-1':   'lua.fs-exists',
  'sec-6-5-2':   'lua.fs-size',
  'sec-6-5-3':   'lua.fs-read',
  'sec-6-5-4':   'lua.fs-write',
  'sec-6-5-5':   'lua.fs-mkdir-p',
  'sec-6-5-6':   'lua.fs-glob',
  'sec-6-5-7':   'lua.fs-mtime',
  'sec-6-6':     'lua.path-helpers',
  'sec-6-6-1':   'lua.path-stem',
  'sec-6-6-2':   'lua.path-name',
  'sec-6-6-3':   'lua.path-ext',
  'sec-6-6-4':   'lua.path-dir',
  'sec-6-6-5':   'lua.path-replace-ext',
  'sec-6-6-6':   'lua.path-join',
  'sec-6-7':     'lua.shell-placeholders',
  'sec-6-8':     'lua.use-env',
  'sec-6-8-1':   'lua.builtin-modules',
  'sec-6-8-2':   'lua.local-modules',

  // ── Chapter 7 — Cross-Cookfile composition ────────────────────────────────
  'sec-7':     'modules',
  'sec-7-1':   'modules.overview',
  'sec-7-2':   'modules.import-declaration',
  'sec-7-3':   'modules.qualified-refs',
  'sec-7-4':   'modules.duplicates-and-cycles',

  // ── Chapter 8 — Execution model ───────────────────────────────────────────
  'sec-8':     'exec',
  'sec-8-1':   'exec.two-phase',
  'sec-8-2':   'exec.capture-mode',
  'sec-8-3':   'exec.step-groups',
  'sec-8-4':   'exec.cross-recipe-ordering',
  'sec-8-5':   'exec.interactive-drain',
  'sec-8-6':   'exec.cache',
  'sec-8-7':   'exec.output-materialisation',
  'sec-8-8':   'exec.diagnostic-ordering',

  // ── Appendix A — Grammar (normative) ──────────────────────────────────────
  'sec-A-1':     'grammar-appendix.top-level',
  'sec-A-2':     'grammar-appendix.declarations',
  'sec-A-3':     'grammar-appendix.recipes',
  'sec-A-3-1':   'grammar-appendix.chore',
  'sec-A-4':     'grammar-appendix.steps',
  'sec-A-5':     'grammar-appendix.primitives',

  // ── Appendix B — Rationale (informative) ──────────────────────────────────
  'sec-B-0':     'rationale.intro',
  'sec-B-0-1':   'rationale.versioning-pre-1-0',
  'sec-B-1':     'rationale.notation',
  'sec-B-2':     'rationale.lexical',
  'sec-B-2-2':   'rationale.one-token-per-line',
  'sec-B-2-3':   'rationale.ascii-identifiers',
  'sec-B-2-4':   'rationale.contextual-keywords',
  'sec-B-2-7':   'rationale.at-not-lexical',
  'sec-B-3':     'rationale.grammar',
  'sec-B-3-2':   'rationale.ordered-prefix',
  'sec-B-3-7':   'rationale.implicit-header-col0',
  'sec-B-3-8':   'rationale.name-value-shell',
  'sec-B-3-10':  'rationale.implicit-end',
  'sec-B-4':     'rationale.recipes',
  'sec-B-4-6':   'rationale.multi-output-using-error',
  // B.4.7 (rationale.plate-single-template) deleted in CS-0024
  'sec-B-4-11':  'rationale.module-call-heuristic',
  'sec-B-4-14':  'rationale.chore-form-not-flag',
  'sec-B-4-15':  'rationale.chore-banned-steps',
  'sec-B-4-16':  'rationale.chore-default-interactive',
  'sec-B-4-17':  'rationale.plate-test-no-output-decl',
  'sec-B-4-18':  'rationale.plate-test-body-deduction',
  'sec-B-4-19':  'rationale.plate-test-out-rejected',
  'sec-B-4-20':  'rationale.plate-test-lib-accessor-rejected',
  'sec-B-4-21':  'rationale.plate-test-lua-static-scan',
  'sec-B-5':     'rationale.exec',
  'sec-B-5-1':   'rationale.register-phase-pure',
  'sec-B-5-2':   'rationale.interactive-drain',
  'sec-B-5-3':   'rationale.abstract-cache',
  'sec-B-5-4':   'rationale.diagnostic-ordering',
  'sec-B-6':     'rationale.lua',
  'sec-B-6-1':   'rationale.placeholder-vs-globals',
  'sec-B-6-2':   'rationale.fs-path-namespaces',
  'sec-B-6-3':   'rationale.add-unit-register-only',
  'sec-B-6-4':   'rationale.recipe-name-priority',
  'sec-B-7':     'rationale.modules',
  'sec-B-7-1':   'rationale.duplicate-import-parse-time',
  'sec-B-7-2':   'rationale.cycle-load-time',
  'sec-B-7-3':   'rationale.module-name-no-collision',

  // ── Appendix C — Worked examples (informative) ────────────────────────────
  'sec-C-1':   'examples.multi-output-cook',
  'sec-C-2':   'examples.cross-recipe-dep',
  'sec-C-3':   'examples.module-use-call',
  'sec-C-4':   'examples.lua-multi-output',
  'sec-C-5':   'examples.cross-recipe-refs',

  // ── Appendix D — Changes (informative) ───────────────────────────────────
  'sec-D-10':  'changes.cs-0010',
  'sec-D-20':  'changes.cs-0020',

  // ── v0.10 reorg: new slug prefixes ────────────────────────────────────────
  // These are stubs that the v0.10 cut populates with real section numbers
  // (the keys remain the positional `sec-N-M-K` form once new chapters are
  // numbered).

  // Ch. 1 — Conformance
  'sec-1-new':           'conf',
  'sec-1-new-1':         'conf.criteria',

  // Ch. 4 — Top-level structure
  'sec-4-new':           'toplevel',
  'sec-4-new-1':         'toplevel.overview',
  'sec-4-new-2':         'toplevel.ordering',
  'sec-4-new-3':         'toplevel.termination',
  'sec-4-new-4':         'toplevel.module-call',

  // Ch. 5 — Declarations
  'sec-5-new':           'decl',
  'sec-5-new-1':         'decl.use',
  'sec-5-new-2':         'decl.import',
  'sec-5-new-3':         'decl.config',
  'sec-5-new-3-1':       'decl.config-composition',
  'sec-5-new-4':         'decl.register',
  'sec-5-new-4-1':       'decl.register-splicing',

  // Ch. 8 — Step kinds
  'sec-8-new':           'steps',
  'sec-8-new-1':         'steps.dispatch',
  'sec-8-new-2':         'steps.ingredients',
  'sec-8-new-3':         'steps.cook-single',
  'sec-8-new-4':         'steps.cook-multi',
  'sec-8-new-5':         'steps.iteration-mode',
  'sec-8-new-6':         'steps.plate',
  'sec-8-new-7':         'steps.test',
  'sec-8-new-8':         'steps.lua',
  'sec-8-new-9':         'steps.shell',

  // Ch. 9 — Placeholders
  'sec-9-new':           'phl',
  'sec-9-new-1':         'phl.token',
  'sec-9-new-2':         'phl.resolution',
  'sec-9-new-3':         'phl.cook-step',
  'sec-9-new-4':         'phl.plate-test',

  // Ch. 11 — Cross-Cookfile composition
  'sec-11-new':          'comp',
  'sec-11-new-1':        'comp.overview',
  'sec-11-new-2':        'comp.import',
  'sec-11-new-3':        'comp.qualified-refs',
  'sec-11-new-4':        'comp.duplicates-and-cycles',

  // Ch. 12 — Modules (use system + catalogue index)
  'sec-12-new':          'mods',
  'sec-12-new-1':        'mods.use',
  'sec-12-new-2':        'mods.use-scope',
  'sec-12-new-3':        'mods.lifecycle',
  'sec-12-new-4':        'mods.builtin',
  'sec-12-new-5':        'mods.local',
  'sec-12-new-6':        'mods.catalogue-index',

  // Ch. 13 — Two-phase model (Part II)
  'sec-13-new':                'exec.phases',
  'sec-13-new-classification': 'exec.phases.classification',

  // Ch. 14 — Capture mode (Part II)
  'sec-14-new':                'exec.capture',

  // Ch. 15 — Step groups and parallelism (Part II)
  'sec-15-new':               'exec.groups',
  'sec-15-new-body-bundling': 'exec.body-bundling',

  // Ch. 20 — Workspace root (Part II)
  'sec-20-new':          'exec.ws',
  'sec-20-new-1':        'exec.ws.determination',
  'sec-17-new-portability': 'exec.cache.portability',

  // Ch. 27 — Catalogue governance
  'sec-27-new':          'cat',
  'sec-27-new-1':        'cat.index',
  'sec-27-new-2':        'cat.bootstrap',
  'sec-27-new-2-1':      'cat.bootstrap.install',
  'sec-27-new-2-2':      'cat.bootstrap.vendor',

  // Ch. 28 — cc module
  'sec-28-new':          'cat.cc',
  'sec-28-new-1':        'cat.cc.synopsis',
  'sec-28-new-2':        'cat.cc.identity',
  'sec-28-new-3':        'cat.cc.surface',
  'sec-28-new-3-1':      'cat.cc.bin',
  'sec-28-new-3-2':      'cat.cc.lib',
  'sec-28-new-3-3':      'cat.cc.shared',
  'sec-28-new-3-4':      'cat.cc.headers',
  'sec-28-new-3-5':      'cat.cc.compile',
  'sec-28-new-3-6':      'cat.cc.archive',
  'sec-28-new-3-7':      'cat.cc.link',
  'sec-28-new-3-8':      'cat.cc.find',
  'sec-28-new-3-8-1':    'cat.cc.find-cmake-compat',
  'sec-28-new-3-8-2':    'cat.cc.find-cmake-compile',
  'sec-28-new-3-8-3':    'cat.cc.find-cmake-link',
  'sec-28-new-3-9':      'cat.cc.defaults',
  'sec-28-new-3-10':     'cat.cc.toolchain',
  'sec-28-new-3-11':     'cat.cc.compile-commands',
  'sec-28-new-3-12':     'cat.cc.register-finder',
  'sec-28-new-3-13':     'cat.cc.find-or-error',
  'sec-28-new-4':        'cat.cc.transitive',
  'sec-28-new-5':        'cat.cc.errors',
  'sec-28-new-6':        'cat.cc.vendoring',

  // Annex D (was E) — Pre-1.0 checklist
  'sec-D-pre-v1':        'pre-v1',
  'sec-D-pre-v1-1':      'pre-v1.parse-txt-coupling',
  'sec-D-pre-v1-2':      'pre-v1.template-vs-bash-expansion',
  'sec-D-pre-v1-3':      'pre-v1.no-string-escape',

  // Annex F — Conformance corpus stub
  'sec-F-corpus':        'corpus',
};
