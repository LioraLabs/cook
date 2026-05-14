// Slug mapping for the Cook Standard.
//
// Keyed by the positional anchor ID (sec-N-M-K). After the v0.10 structural
// reorg, this file remains in-tree as the authoritative registry of chapter
// prefixes and their clauses — future renames update it alongside the heading
// markers.
//
// Retired slug families (`grammar.*`, `modules.*`, `stdmods.*`,
// `lexical.placeholders`, `lua.shell-placeholders`, `lua.use-env`,
// `lua.builtin-modules`, `lua.local-modules`, `recipes.body-bundling`,
// `intro.conformance`) are intentionally NOT present here — those map through
// `scripts/slug-renames.ts` instead.

export const SLUG_MAPPING: Record<string, string> = {
  // ── Chapter 0 — Introduction ──────────────────────────────────────────────
  'sec-0':     'intro',
  'sec-0-1':   'intro.purpose',
  'sec-0-2':   'intro.scope',
  'sec-0-3':   'intro.non-scope',
  'sec-0-4':   'intro.normative-informative',
  'sec-0-5':   'intro.version',
  'sec-0-6':   'intro.architecture-relationship',

  // ── Chapter 1 — Conformance ────────────────────────────────────────────────
  'sec-1':     'conf',
  'sec-1-1':   'conf.criteria',

  // ── Chapter 2 — Notation and conventions ──────────────────────────────────
  'sec-2':     'notation',
  'sec-2-1':   'notation.keywords',
  'sec-2-2':   'notation.numbering-and-citation',
  'sec-2-3':   'notation.normative-blocks',
  'sec-2-4':   'notation.grammar',
  'sec-2-5':   'notation.grammar-precedence',
  'sec-2-6':   'notation.amendment-markers',
  'sec-2-7':   'notation.stable-anchors',

  // ── Chapter 3 — Lexical structure ─────────────────────────────────────────
  'sec-3':       'lexical',
  'sec-3-1':     'lexical.source-representation',
  'sec-3-2':     'lexical.tokens',
  'sec-3-3':     'lexical.identifiers',
  'sec-3-4':     'lexical.keywords',
  'sec-3-5':     'lexical.strings',
  'sec-3-6':     'lexical.comments',
  'sec-3-7':     'lexical.line-prefixes',
  'sec-3-8':     'lexical.numbers',
  'sec-3-9':     'lexical.brace-blocks',
  'sec-3-9-1':   'lexical.brace-blocks.lua-spans',
  'sec-3-9-2':   'lexical.brace-blocks.shell-spans',
  'sec-3-10':    'lexical.line-classification',

  // ── Chapter 4 — Top-level structure ───────────────────────────────────────
  'sec-4':       'toplevel',
  'sec-4-1':     'toplevel.overview',
  'sec-4-2':     'toplevel.ordering',
  'sec-4-3':     'toplevel.termination',
  'sec-4-4':     'toplevel.module-call',

  // ── Chapter 5 — Declarations ──────────────────────────────────────────────
  'sec-5':       'decl',
  'sec-5-1':     'decl.use',
  'sec-5-2':     'decl.import',
  'sec-5-3':     'decl.config',
  'sec-5-3-1':   'decl.config-composition',
  'sec-5-4':     'decl.register',
  'sec-5-4-1':   'decl.register-splicing',

  // ── Chapter 6 — Recipes ───────────────────────────────────────────────────
  'sec-6':       'recipes',
  'sec-6-1':     'recipes.header-forms',
  'sec-6-2':     'recipes.dep-list',
  'sec-6-3':     'recipes.region-rule',

  // ── Chapter 7 — Chores ────────────────────────────────────────────────────
  'sec-7':       'chores',
  'sec-7-1':     'chores.header',
  'sec-7-2':     'chores.body',
  'sec-7-3':     'chores.default-interactive',
  'sec-7-4':     'chores.no-caching',
  'sec-7-5':     'chores.cross-form-deps',

  // ── Chapter 8 — Step kinds ────────────────────────────────────────────────
  'sec-8':       'steps',
  'sec-8-1':     'steps.dispatch',
  'sec-8-2':     'steps.ingredients',
  'sec-8-3':     'steps.overview',
  'sec-8-4':     'steps.cook-single',
  'sec-8-4-1':   'steps.iteration-mode',
  'sec-8-5':     'steps.cook-multi',
  'sec-8-6':     'steps.plate',
  'sec-8-6-1':   'steps.iteration-mode-plate-test',
  'sec-8-7':     'steps.test',
  'sec-8-8':     'steps.lua',
  'sec-8-8-1':   'steps.lua-delimitation',
  'sec-8-8-2':   'steps.lua-execution-phase',
  'sec-8-8-3':   'steps.lua-examples',
  'sec-8-9':     'steps.shell',

  // ── Chapter 9 — Placeholders ──────────────────────────────────────────────
  'sec-9':       'phl',
  'sec-9-1':     'phl.token',
  'sec-9-2':     'phl.resolution',
  'sec-9-3':     'phl.cook-step',
  'sec-9-4':     'phl.plate-test',

  // ── Chapter 10 — Cross-recipe references ──────────────────────────────────
  'sec-10':       'xref',
  'sec-10-1':     'xref.name-references',
  'sec-10-2':     'xref.resolution',
  'sec-10-2-1':   'xref.dotted-names',
  'sec-10-2-2':   'xref.reserved-segment',
  'sec-10-2-3':   'xref.env-shadowing',
  'sec-10-3':     'xref.path-accessors',
  'sec-10-4':     'xref.dep-driven',
  'sec-10-4-1':   'xref.dep-recipe-output',
  'sec-10-4-2':   'xref.dep-driven-example',
  'sec-10-5':     'xref.string-substitution',
  'sec-10-5-1':   'xref.string-substitution-example',
  'sec-10-6':     'xref.dep-implications',
  'sec-10-7':     'xref.env-namespace',

  // ── Chapter 11 — Cross-Cookfile composition ──────────────────────────────
  'sec-11':      'comp',
  'sec-11-1':    'comp.overview',
  'sec-11-2':    'comp.import',
  'sec-11-3':    'comp.qualified-refs',
  'sec-11-4':    'comp.use-scope-pointer',
  'sec-11-5':    'comp.duplicates-and-cycles',

  // ── Chapter 12 — Modules ──────────────────────────────────────────────────
  'sec-12':      'mods',
  'sec-12-1':    'mods.use',
  'sec-12-2':    'mods.use-scope',
  'sec-12-3':    'mods.lifecycle',
  'sec-12-3-1':  'mods.lifecycle.load-order',
  'sec-12-3-2':  'mods.lifecycle.caching',
  'sec-12-3-3':  'mods.lifecycle.cycles',
  'sec-12-3-4':  'mods.lifecycle.rehydration',
  'sec-12-4':    'mods.builtin',
  'sec-12-5':    'mods.local',
  'sec-12-6':    'mods.catalogue-index',

  // ── Chapter 13 — Two-phase model ─────────────────────────────────────────
  'sec-13':      'exec.phases',
  'sec-13-1':    'exec.two-phase',
  'sec-13-2':    'exec.phases.classification',

  // ── Chapter 14 — Capture mode ────────────────────────────────────────────
  'sec-14':      'exec.capture',
  'sec-14-1':    'exec.capture-mode',

  // ── Chapter 15 — Step groups and parallelism ─────────────────────────────
  'sec-15':      'exec.groups',
  'sec-15-1':    'exec.step-groups',
  'sec-15-2':    'exec.body-bundling',

  // ── Chapter 16 — Cross-recipe ordering and drain ─────────────────────────
  'sec-16':      'exec.ord',
  'sec-16-1':    'exec.cross-recipe-ordering',
  'sec-16-1-1':  'exec.output-uniqueness',
  'sec-16-2':    'exec.interactive-drain',

  // ── Chapter 17 — Cache semantics ─────────────────────────────────────────
  'sec-17':      'exec.cache',
  'sec-17-1':    'exec.cache.abstract',
  'sec-17-1-1':  'exec.cache.tool-binary',
  'sec-17-2':    'exec.cache.integrity',
  'sec-17-3':    'exec.cache.discovered-inputs',
  'sec-17-4':    'exec.cache.test-unit',
  'sec-17-5':    'exec.cache.portability',

  // ── Chapter 18 — Output materialisation ──────────────────────────────────
  'sec-18':      'exec.mat',
  'sec-18-1':    'exec.output-materialisation',

  // ── Chapter 19 — Diagnostic ordering ─────────────────────────────────────
  'sec-19':      'exec.diag',
  'sec-19-1':    'exec.diagnostic-ordering',

  // ── Chapter 20 — Workspace root ──────────────────────────────────────────
  'sec-20':      'exec.ws',
  'sec-20-1':    'exec.ws.determination',

  // ── Chapter 21 — Cook Lua API surface overview ──────────────────────────
  'sec-21':      'lua',
  'sec-21-1':    'lua.api-overview',
  'sec-21-2':    'lua.recipe-global',
  'sec-21-3':    'lua.env-alias',

  // ── Chapter 22 — Register-phase API ─────────────────────────────────────
  'sec-22':      'lua.reg',
  'sec-22-1':    'lua.add-unit',
  'sec-22-1-1':  'lua.add-unit-discovered-inputs',
  'sec-22-2':    'lua.cook-exec',
  'sec-22-3':    'lua.cook-recipe',
  'sec-22-4':    'lua.cook-add-test',
  'sec-22-5':    'lua.step-group',

  // ── Chapter 23 — Execute-phase API ──────────────────────────────────────
  'sec-23':      'lua.exe',
  'sec-23-1':    'lua.using-block-globals',
  'sec-23-2':    'lua.using-block-globals-plate-test',

  // ── Chapter 24 — Both-phase API ─────────────────────────────────────────
  'sec-24':      'lua.both',
  'sec-24-1':    'lua.cook-sh',
  'sec-24-2':    'lua.cook-load-module',
  'sec-24-3':    'lua.cook-env',
  'sec-24-4':    'lua.cook-cache',
  'sec-24-4-1':  'lua.cook-cache-get',
  'sec-24-4-2':  'lua.cook-cache-set',
  'sec-24-4-3':  'lua.cook-cache-scope',
  'sec-24-5':    'lua.cook-export-import',
  'sec-24-5-1':  'lua.cook-export',
  'sec-24-5-2':  'lua.cook-import',
  'sec-24-6':    'lua.cook-platform',
  'sec-24-7':    'lua.cook-dep-output',
  'sec-24-7-1':  'lua.cook-dep-output-single',
  'sec-24-7-2':  'lua.cook-dep-output-list',

  // ── Chapter 25 — fs.* (incl. sandbox) ────────────────────────────────────
  'sec-25':      'lua.fs',
  'sec-25-1':    'lua.fs-helpers',
  'sec-25-2':    'lua.fs-exists',
  'sec-25-3':    'lua.fs-size',
  'sec-25-4':    'lua.fs-read',
  'sec-25-5':    'lua.fs-write',
  'sec-25-6':    'lua.fs-mkdir-p',
  'sec-25-7':    'lua.fs-glob',
  'sec-25-8':    'lua.fs-mtime',
  'sec-25-9':    'lua.fs-sandbox',
  'sec-25-10':   'lua.shell-escape-hatches',

  // ── Chapter 26 — path.* ──────────────────────────────────────────────────
  'sec-26':      'lua.path',
  'sec-26-1':    'lua.path-helpers',
  'sec-26-2':    'lua.path-stem',
  'sec-26-3':    'lua.path-name',
  'sec-26-4':    'lua.path-ext',
  'sec-26-5':    'lua.path-dir',
  'sec-26-6':    'lua.path-replace-ext',
  'sec-26-7':    'lua.path-join',

  // ── Chapter 27 — Catalogue governance ────────────────────────────────────
  'sec-27':      'cat',
  'sec-27-1':    'cat.bootstrap',
  'sec-27-1-1':  'cat.bootstrap.install',
  'sec-27-1-2':  'cat.bootstrap.vendor',
  'sec-27-2':    'cat.index',

  // ── Chapter 28 — cc module ───────────────────────────────────────────────
  'sec-28':           'cat.cc',
  'sec-28-1':         'cat.cc.synopsis',
  'sec-28-2':         'cat.cc.identity',
  'sec-28-3':         'cat.cc.surface',
  'sec-28-3-1':       'cat.cc.bin',
  'sec-28-3-2':       'cat.cc.lib',
  'sec-28-3-3':       'cat.cc.shared',
  'sec-28-3-4':       'cat.cc.headers',
  'sec-28-3-5':       'cat.cc.compile',
  'sec-28-3-6':       'cat.cc.archive',
  'sec-28-3-7':       'cat.cc.link',
  'sec-28-3-8':       'cat.cc.find',
  'sec-28-3-8-1':     'cat.cc.find-cmake-compat',
  'sec-28-3-8-2':     'cat.cc.find-cmake-compile',
  'sec-28-3-8-3':     'cat.cc.find-cmake-link',
  'sec-28-3-9':       'cat.cc.defaults',
  'sec-28-3-10':      'cat.cc.toolchain',
  'sec-28-3-11':      'cat.cc.compile-commands',
  'sec-28-3-12':      'cat.cc.register-finder',
  'sec-28-3-13':      'cat.cc.find-or-error',
  'sec-28-4':         'cat.cc.transitive',
  'sec-28-5':         'cat.cc.errors',
  'sec-28-6':         'cat.cc.vendoring',

  // ── Appendix A — Grammar (normative) ──────────────────────────────────────
  'sec-A-1':     'grammar-appendix.top-level',
  'sec-A-2':     'grammar-appendix.declarations',
  'sec-A-3':     'grammar-appendix.recipes',
  'sec-A-3-1':   'grammar-appendix.chore',
  'sec-A-4':     'grammar-appendix.steps',
  'sec-A-5':     'grammar-appendix.primitives',

  // ── Appendix B — Worked examples (informative) ────────────────────────────
  'sec-B-1':   'examples.multi-output-cook',
  'sec-B-2':   'examples.cross-recipe-dep',
  'sec-B-3':   'examples.module-use-call',
  'sec-B-4':   'examples.lua-multi-output',
  'sec-B-5':   'examples.cross-recipe-refs',

  // ── Appendix C — Rationale (informative) ──────────────────────────────────
  'sec-C-0':     'rationale.intro',
  'sec-C-0-1':   'rationale.versioning-pre-1-0',
  'sec-C-1':     'rationale.notation',
  'sec-C-2':     'rationale.lexical',
  'sec-C-2-2':   'rationale.one-token-per-line',
  'sec-C-2-3':   'rationale.ascii-identifiers',
  'sec-C-2-4':   'rationale.contextual-keywords',
  'sec-C-2-7':   'rationale.at-not-lexical',
  'sec-C-3':     'rationale.grammar',
  'sec-C-3-2':   'rationale.ordered-prefix',
  'sec-C-3-7':   'rationale.implicit-header-col0',
  'sec-C-3-8':   'rationale.name-value-shell',
  'sec-C-3-10':  'rationale.implicit-end',
  'sec-C-4':     'rationale.recipes',
  'sec-C-4-6':   'rationale.multi-output-using-error',
  // C.4.7 (rationale.plate-single-template) deleted in CS-0024
  'sec-C-4-11':  'rationale.module-call-heuristic',
  'sec-C-4-14':  'rationale.chore-form-not-flag',
  'sec-C-4-15':  'rationale.chore-banned-steps',
  'sec-C-4-16':  'rationale.chore-default-interactive',
  'sec-C-4-17':  'rationale.plate-test-no-output-decl',
  'sec-C-4-18':  'rationale.plate-test-body-deduction',
  'sec-C-4-19':  'rationale.plate-test-out-rejected',
  'sec-C-4-20':  'rationale.plate-test-lib-accessor-rejected',
  'sec-C-4-21':  'rationale.plate-test-lua-static-scan',
  'sec-C-5':     'rationale.exec',
  'sec-C-5-1':   'rationale.register-phase-pure',
  'sec-C-5-2':   'rationale.interactive-drain',
  'sec-C-5-3':   'rationale.abstract-cache',
  'sec-C-5-4':   'rationale.diagnostic-ordering',
  'sec-C-6':     'rationale.lua',
  'sec-C-6-1':   'rationale.placeholder-vs-globals',
  'sec-C-6-2':   'rationale.fs-path-namespaces',
  'sec-C-6-3':   'rationale.add-unit-register-only',
  'sec-C-6-4':   'rationale.recipe-name-priority',
  'sec-C-7':     'rationale.modules',
  'sec-C-7-1':   'rationale.duplicate-import-parse-time',
  'sec-C-7-2':   'rationale.cycle-load-time',
  'sec-C-7-3':   'rationale.module-name-no-collision',

  // ── Appendix D — Pre-1.0 checklist ────────────────────────────────────────
  'sec-D-pre-v1':    'pre-v1',
  'sec-D-pre-v1-1':  'pre-v1.parse-txt-coupling',
  'sec-D-pre-v1-2':  'pre-v1.template-vs-bash-expansion',
  'sec-D-pre-v1-3':  'pre-v1.no-string-escape',

  // ── Appendix E — Changes (informative) ───────────────────────────────────
  'sec-E-10':  'changes.cs-0010',
  'sec-E-20':  'changes.cs-0020',

  // ── Appendix F — Conformance corpus stub ──────────────────────────────────
  'sec-F-corpus':    'corpus',
};
