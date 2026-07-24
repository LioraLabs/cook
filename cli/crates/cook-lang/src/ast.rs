use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub struct UseStatement {
    pub module_name: String,
    pub line: usize,
}

/// The shape of an import path token (§7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPath {
    /// Tree-relative path: forward-only, no `..`, not absolute.
    /// Resolves relative to the importing Cookfile's directory.
    Tree(String),
    /// Sigil-anchored path: begins with `//`. The stored String is the
    /// path AFTER the sigil (forward-only, no `..`, no leading `/`).
    /// Resolves relative to the workspace root.
    Sigil(String),
}

impl std::fmt::Display for ImportPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportPath::Tree(s) => f.write_str(s),
            ImportPath::Sigil(s) => write!(f, "//{s}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDecl {
    pub name: String,
    pub path: ImportPath,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigBlock {
    pub name: Option<String>,
    pub body: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegisterBlock {
    pub body: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopLevelModuleCall {
    /// The collected call source — possibly multi-line if the call's brace
    /// span extends beyond its first line (collected via the existing
    /// `collect_module_call` brace-balance machinery). Whitespace and
    /// comments inside the call body are preserved verbatim.
    pub code: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChoreParam {
    Required { name: String, line: usize, col: usize },
    DefaultedString { name: String, default: String, line: usize, col: usize },
    DefaultedLua { name: String, default_lua: String, line: usize, col: usize },
    VariadicPlus { name: String, line: usize, col: usize },
    VariadicStar { name: String, line: usize, col: usize },
}

impl ChoreParam {
    pub fn name(&self) -> &str {
        match self {
            ChoreParam::Required { name, .. }
            | ChoreParam::DefaultedString { name, .. }
            | ChoreParam::DefaultedLua { name, .. }
            | ChoreParam::VariadicPlus { name, .. }
            | ChoreParam::VariadicStar { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chore {
    pub name: String,
    pub params: Vec<ChoreParam>,
    pub deps: Vec<String>,
    pub steps: Vec<Step>,
    pub line: usize,
}

/// A `probe` declaration (§22.5). Native surface sugar over the register-phase
/// `cook.probe()` API: lowering (COOK-68) emits the equivalent `cook.probe`
/// call. `deps` is the make-style header dependency list (`probe N: a b`) and
/// lowers to `inputs.requires`. `ingredients`/`excludes` are the file-input
/// fingerprint set (NOT an iteration driver — a probe yields one value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub name: String,
    pub deps: Vec<String>,
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,
    pub produce: ProbeProduce,
    pub line: usize,
}

/// The value-producing body of a probe (§22.5). The producer KIND leads the
/// brace body; there are no `produce`/`as` tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeProduce {
    /// `>{ … }` — Lua block; value is the block's `return`.
    Lua(String),
    /// `{ … }` / `json { … }` / `lines { … }` — shell block (bare) or typed;
    /// value is stdout, typed by the leading kind keyword.
    Shell { commands: Vec<String>, typing: ShellProduceType },
    /// `tools { cc, ld }` — the brace content is a LIST of bare tool names
    /// (NOT a shell body). Each is PATH-resolved and its binary hashed; the
    /// value is `{ NAME = { path, hash }, … }`. The hash is both the value and
    /// the re-run trigger (COOK-164).
    Tools(Vec<String>),
    /// `envs { CFLAGS, LDFLAGS }` — the brace content is a LIST of bare
    /// env-var names (NOT a shell body). The value is `{ NAME = value_or_nil,
    /// … }`; reading the env records the consulted-env determinant (COOK-164).
    Envs(Vec<String>),
    /// `files { "src/*.ts" !"src/gen/*.ts" }` — the brace content is a LIST of
    /// quoted glob patterns (NOT a shell body), `!"…"` excluding, following
    /// `ingredients` pattern syntax. The expanded file set self-fingerprints
    /// and the value is `{ [path] = content_hash, … }` — per-file identity as
    /// a sealable determinant (CS-0148). A `files` probe MUST NOT also declare
    /// an `ingredients` line: the glob set IS its file-input fingerprint set.
    Files { globs: Vec<String>, excludes: Vec<String> },
}

/// How a shell-block probe's stdout becomes the probe value (§22.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellProduceType {
    /// Bare `{ … }` — stdout as a string (one trailing newline trimmed).
    String,
    /// `lines { … }` — stdout split on LF into an array.
    Lines,
    /// `json { … }` — stdout parsed as one JSON value.
    Json,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cookfile {
    pub config_blocks: Vec<ConfigBlock>,
    pub recipes: Vec<Recipe>,
    pub chores: Vec<Chore>,
    pub uses: Vec<UseStatement>,
    pub imports: Vec<ImportDecl>,
    pub register_blocks: Vec<RegisterBlock>,
    pub top_level_module_calls: Vec<TopLevelModuleCall>,
    pub probes: Vec<Probe>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub name: String,
    pub deps: Vec<String>,
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,
    pub steps: Vec<Step>,
    pub line: usize,
}

/// A step body — the `body` production from App. A.4 (CS-0024): either a
/// `{ … }` shell block or a `>{ … }` execute-phase Lua block. Shared by
/// `cook_step` (CS-0099: the body follows the output pattern(s) directly)
/// and `test_step` so the codegen can share substitution / mode detection
/// helpers without duplicating the enum.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    ShellBlock(Vec<String>),
    LuaBlock(String),
}

/// An output pattern in a `cook OUT [OUT...] body` step.
///
/// The output slot accepts either a literal quoted pattern (with `$<...>`
/// sigil substitution) or — under §8.4.2's one-to-one form — a single
/// parenthesised Lua expression evaluated per-ingredient. CS-0089 / COOK-59.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputPattern {
    /// Quoted string with `$<...>` sigils (the historical form).
    Quoted(String),
    /// Parenthesised Lua expression (`cook (EXPR) >{ … }`). Evaluated
    /// per-ingredient at register time with `input` bound to the current
    /// ingredient's path. Standard §8.4.2.
    LuaExpr(String),
}

impl OutputPattern {
    /// The underlying pattern source string (either the quoted template
    /// or the Lua expression text). Use this when callers don't need to
    /// distinguish kinds — e.g. for diagnostics or for the existing
    /// pre-Task-3 codegen path that only handles `Quoted` patterns.
    pub fn as_str(&self) -> &str {
        match self {
            OutputPattern::Quoted(s) | OutputPattern::LuaExpr(s) => s.as_str(),
        }
    }

    /// `true` if this is the parenthesised Lua-expression form.
    pub fn is_lua_expr(&self) -> bool {
        matches!(self, OutputPattern::LuaExpr(_))
    }
}

impl From<&str> for OutputPattern {
    fn from(s: &str) -> Self {
        OutputPattern::Quoted(s.to_string())
    }
}

impl From<String> for OutputPattern {
    fn from(s: String) -> Self {
        OutputPattern::Quoted(s)
    }
}

/// Cache disposition of a `cook` step (Cache-trust v3, §8.4.3). All-default
/// = unannotated. Parser/grammar surface only; the cache-key effect lands in
/// COOK-161/162/163.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Disposition {
    /// `local` / `pinned` sharing — mutually exclusive, so modelled as one
    /// `Sharing` enum (`(local, pinned) = (true, true)` is unrepresentable).
    pub sharing: cook_contracts::Sharing,
    /// `record` — record divergence for the oracle. Orthogonal to `sharing`.
    pub record: bool,
    /// `seal` refs — sorted, de-duplicated bare probe keys (additive).
    pub seal: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CookStep {
    pub outputs: Vec<OutputPattern>,
    pub body: Option<Body>,
    pub disposition: Disposition,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TestStep {
    pub body: Body,
    /// Effective `seal` refs — sorted, de-duplicated bare probe keys. Folded
    /// from the recipe-level `seal` baseline plus this test's trailing
    /// `seal`/`unseal` tail (§8.4.3, CS-0159). A test unit is a cacheable
    /// unit, so it keys on its sealed probes' values exactly as a `cook`
    /// unit does (§17.4 rule 1).
    ///
    /// Unlike [`CookStep`], a test carries no `share_mod` — `local` /
    /// `pinned` / `nondet` state facts about an *output artifact*, and a test
    /// produces a pass/fail record rather than artifacts. The test tail is
    /// therefore the input half of `cook_mods` only.
    pub seal: BTreeSet<String>,
}

/// The source of a `for_each` step's data members. Only a probe-key source
/// remains after COOK-97: the `$(cmd)` shell-capture and `(LUA_EXPR)` reserved
/// forms have been removed. CS-0091 / COOK-62 introduced the node; COOK-97
/// drops the non-probe variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForEachSource {
    /// A probe key, optionally selecting a nested array field (`cards`,
    /// `cards:items`). The probe's value MUST be an array (§22.5.10).
    ProbeKey(String),
}

/// A `for_each` step — the internal `ingredients <probe>` desugar node (§8.x).
/// At most one per recipe; mutually exclusive with `ingredients`. The current
/// member binds as `$<in>` / `$<in.field>`. Source is always a `ProbeKey`
/// (the `$(cmd)` shell-capture and `(LUA_EXPR)` anonymous-source forms were
/// removed in COOK-97; see §8.x and CS-NNNN).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForEachStep {
    pub source: ForEachSource,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Step {
    Shell { command: String, line: usize, interactive: bool },
    /// Execute-phase Lua line (`>` prefix). Coalesced into a body unit by
    /// codegen; runs on the worker VM at execute time.
    Lua { code: String, line: usize },
    /// Execute-phase Lua block (`>{ … }` prefix). Same execution model as `Lua`.
    LuaBlock { code: String, line: usize },
    /// Register-phase inline Lua. Produced by an auto-classified bare
    /// module-call line (`ident.ident(...)`) in a recipe body (CS-0134);
    /// formerly also by the removed `>>` prefix.
    InlineLua { code: String, line: usize },
    Cook { step: CookStep, line: usize },
    Test { step: TestStep, line: usize },
    /// Register-phase data-member iteration driver (§8.3). Declarative.
    ForEach { step: ForEachStep, line: usize },
}

#[cfg(test)]
#[path = "tests/ast_tests.rs"]
mod tests;
