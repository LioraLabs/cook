// Slug renames for the Cook Standard v0.10 structural redesign.
//
// One-way map from a retired slug to its replacement. The Astro build
// reads this map to emit redirects from old anchored URLs to new ones,
// and the remark-slug-xrefs plugin consults it on a missing slug to
// emit a precise rename error.
//
// Keep entries in retired-slug alphabetical order.

export const SLUG_RENAMES: Record<string, string | null> = {
  // null means: retired with no replacement (already removed from the
  // language by a prior CS entry).

  'exec':                            'exec.phases',
  'grammar':                         'toplevel.overview',
  'grammar.overview':                'toplevel.overview',
  'grammar.top-level-ordering':      'toplevel.ordering',
  'grammar.var-declarations':        null,
  'grammar.use-declarations':        'decl.use',
  'grammar.import-declarations':     'decl.import',
  'grammar.config-blocks':           'decl.config',
  'grammar.config-composition':      'decl.config-composition',
  'grammar.register-blocks':         'decl.register',
  'grammar.register-blocks.splicing':'decl.register-splicing',
  'grammar.recipe-syntax':           'recipes.header-forms',
  'grammar.step-dispatch':           'steps.dispatch',
  'grammar.top-level-module-call':   'toplevel.module-call',

  'modules':                         'comp.overview',
  'modules.overview':                'comp.overview',
  'modules.import-declaration':      'comp.import',
  'modules.qualified-refs':          'comp.qualified-refs',
  'modules.use-scope':               'mods.use-scope',
  'modules.duplicates-and-cycles':   'comp.duplicates-and-cycles',
  'modules.workspace-root':          'exec.ws.determination',
  'modules.cache-invariants':        'exec.cache.portability',

  'stdmods':                         'cat.bootstrap',
  'stdmods.bootstrap':               'cat.bootstrap',
  'stdmods.bootstrap.install':       'cat.bootstrap.install',
  'stdmods.bootstrap.vendor':        'cat.bootstrap.vendor',
  'stdmods.bootstrap.catalogue':     'cat.index',
  'stdmods.cc':                      'cat.cc',
  'stdmods.cc.synopsis':             'cat.cc.synopsis',
  'stdmods.cc.identity':             'cat.cc.identity',
  'stdmods.cc.surface':              'cat.cc.surface',
  'stdmods.cc.bin':                  'cat.cc.bin',
  'stdmods.cc.lib':                  'cat.cc.lib',
  'stdmods.cc.shared':               'cat.cc.shared',
  'stdmods.cc.headers':              'cat.cc.headers',
  'stdmods.cc.compile':              'cat.cc.compile',
  'stdmods.cc.archive':              'cat.cc.archive',
  'stdmods.cc.link':                 'cat.cc.link',
  'stdmods.cc.find':                 'cat.cc.find',
  'stdmods.cc.find-cmake-compat':    'cat.cc.find-cmake-compat',
  'stdmods.cc.find-cmake-compile':   'cat.cc.find-cmake-compile',
  'stdmods.cc.find-cmake-link':      'cat.cc.find-cmake-link',
  'stdmods.cc.defaults':             'cat.cc.defaults',
  'stdmods.cc.toolchain':            'cat.cc.toolchain',
  'stdmods.cc.compile-commands':     'cat.cc.compile-commands',
  'stdmods.cc.register-finder':      'cat.cc.register-finder',
  'stdmods.cc.find-or-error':        'cat.cc.find-or-error',
  'stdmods.cc.transitive':           'cat.cc.transitive',
  'stdmods.cc.errors':               'cat.cc.errors',
  'stdmods.cc.vendoring':            'cat.cc.vendoring',

  'lexical.placeholders':            'phl.token',

  'recipes.body-bundling':           'exec.body-bundling',
  'recipes.termination':             'toplevel.termination',

  'exec.cache.tool-binary':          'exec.cache.single-key',
  'exec.phase-classification':       'exec.phases.classification',

  'lua.shell-placeholders':            'phl.cook-step',
  'lua.shell-placeholders-plate-test': 'phl.plate-test',

  'lua.use-env':                     'mods.use',
  'lua.builtin-modules':             'mods.builtin',
  'lua.local-modules':               'mods.local',

  // Chapter 4 (legacy "Recipes and step kinds") split into Ch. 6 (recipe header)
  // and Ch. 8 (step kinds) in the v0.10 reorg. Slugs that lived on step bodies
  // moved with them.
  'recipes.cook-single-output':      'steps.cook-single',
  'recipes.cook-multi-output':       'steps.cook-multi',
  'recipes.plate-step':              'steps.plate',
  'recipes.test-step':               'steps.test',
  'recipes.lua-steps':               'steps.lua',
  'recipes.shell-steps':             'steps.shell',
  'recipes.module-call-steps':       'toplevel.module-call',
  'recipes.ingredients':             'steps.ingredients',
  'recipes.step-kinds':              'steps.overview',
  'recipes.iteration-mode-plate-test':'steps.iteration-mode-plate-test',
  'recipes.iteration-mode':          'steps.iteration-mode',
  'recipes.plate-step-not-sandboxed':'steps.plate',

  'intro.conformance':               'conf.criteria',
};

export function resolveRename(retired: string): string | null | undefined {
  return Object.prototype.hasOwnProperty.call(SLUG_RENAMES, retired)
    ? SLUG_RENAMES[retired]
    : undefined;
}
