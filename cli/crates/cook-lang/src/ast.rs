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

#[derive(Debug, Clone, PartialEq)]
pub struct Cookfile {
    pub config_blocks: Vec<ConfigBlock>,
    pub recipes: Vec<Recipe>,
    pub chores: Vec<Chore>,
    pub uses: Vec<UseStatement>,
    pub imports: Vec<ImportDecl>,
    pub register_blocks: Vec<RegisterBlock>,
    pub top_level_module_calls: Vec<TopLevelModuleCall>,
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

#[derive(Debug, Clone, PartialEq)]
pub enum UsingClause {
    ShellBlock(Vec<String>),
    LuaBlock(String),
}

/// CS-0024: a step body — same grammar as `using_clause`'s payload.
/// Used by `cook_step` (via `UsingClause`), `plate_step`, and `test_step`.
/// Aliased to `UsingClause` so the codegen can share substitution / mode
/// detection helpers without duplicating the enum.
pub type Body = UsingClause;

/// An output pattern in a `cook OUT [OUT...] using ...` step.
///
/// The output slot accepts either a literal quoted pattern (with `$<...>`
/// sigil substitution) or — under §8.4.2's one-to-one form — a single
/// parenthesised Lua expression evaluated per-ingredient. CS-0089 / COOK-59.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputPattern {
    /// Quoted string with `$<...>` sigils (the historical form).
    Quoted(String),
    /// Parenthesised Lua expression (`cook (EXPR) using ...`). Evaluated
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

#[derive(Debug, Clone, PartialEq)]
pub struct CookStep {
    pub outputs: Vec<OutputPattern>,
    pub using_clause: Option<UsingClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlateStep {
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TestStep {
    pub body: Body,
    pub as_name: Option<String>,
    pub timeout: Option<u64>,
    pub should_fail: bool,
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
    /// Register-phase Lua line (`>>` prefix). Inlined into the recipe-body Lua
    /// function; runs during registration.
    InlineLua { code: String, line: usize },
    /// Register-phase Lua block (`>>{ … }` prefix). Same registration model
    /// as `InlineLua`. Module-call lines also desugar to `InlineLua` /
    /// `InlineLuaBlock` per §{recipes.module-call-steps}.
    InlineLuaBlock { code: String, line: usize },
    Cook { step: CookStep, line: usize },
    Plate { step: PlateStep, line: usize },
    Test { step: TestStep, line: usize },
}

impl Step {
    /// Phase classification of this step (§{exec.phase-classification}).
    /// Used by the recipe-body region rule (App. A.3) to detect
    /// imperative-then-declarative ordering violations.
    pub fn is_imperative(&self) -> bool {
        matches!(
            self,
            Step::Shell { .. } | Step::Lua { .. } | Step::LuaBlock { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recipe_construction() {
        let recipe = Recipe {
            name: "build".to_string(),
            deps: vec!["setup".to_string()],
            ingredients: vec!["src/*.c".to_string()],
            excludes: vec![],
            steps: vec![
                Step::Cook {
                    step: CookStep {
                        outputs: vec![OutputPattern::Quoted("build/obj/{stem}.o".to_string())],
                        using_clause: Some(UsingClause::ShellBlock(
                            vec!["gcc -c {in} -o {out}".to_string()],
                        )),
                    },
                    line: 4,
                },
            ],
            line: 1,
        };
        assert_eq!(recipe.name, "build");
        assert_eq!(recipe.deps, vec!["setup"]);
        assert_eq!(recipe.steps.len(), 1);
    }

    #[test]
    fn test_recipe_no_metadata() {
        let recipe = Recipe {
            name: "clean".to_string(),
            deps: vec![],
            ingredients: vec![],
            excludes: vec![],
            steps: vec![Step::Shell {
                command: "rm -rf build".to_string(),
                line: 2,
                interactive: false,
            }],
            line: 1,
        };
        assert!(recipe.deps.is_empty());
        assert!(recipe.ingredients.is_empty());
    }

    #[test]
    fn test_cook_step_declaration_only() {
        let step = CookStep {
            outputs: vec![OutputPattern::Quoted("bin/app".to_string())],
            using_clause: None,
        };
        assert!(step.using_clause.is_none());
    }

    #[test]
    fn test_cook_step_lua_block() {
        let step = CookStep {
            outputs: vec![OutputPattern::Quoted("build/obj/{stem}.o".to_string())],
            using_clause: Some(UsingClause::LuaBlock(
                "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
            )),
        };
        assert!(matches!(step.using_clause, Some(UsingClause::LuaBlock(_))));
    }

    #[test]
    fn test_plate_step() {
        let _step = PlateStep {
            body: Body::ShellBlock(vec!["./{in}".to_string()]),
        };
    }

    #[test]
    fn test_interactive_shell_step() {
        let step = Step::Shell {
            command: "./bin/app".to_string(),
            line: 2,
            interactive: true,
        };
        match step {
            Step::Shell { interactive, .. } => assert!(interactive),
            _ => panic!("expected Shell step"),
        }
    }

    #[test]
    fn test_cookfile_with_uses() {
        let cookfile = Cookfile {
            config_blocks: vec![],
            recipes: vec![],
            chores: vec![],
            uses: vec![UseStatement { module_name: "cpp".to_string(), line: 1 }],
            imports: vec![],
            register_blocks: vec![],
            top_level_module_calls: vec![],
        };
        assert_eq!(cookfile.uses.len(), 1);
        assert_eq!(cookfile.uses[0].module_name, "cpp");
    }

    #[test]
    fn test_config_block_construction() {
        let block = ConfigBlock {
            name: Some("release".to_string()),
            body: "env.CXXFLAGS = \"-O3\"".to_string(),
            line: 3,
        };
        assert_eq!(block.name.as_deref(), Some("release"));
        assert!(block.body.contains("CXXFLAGS"));
        assert_eq!(block.line, 3);
    }

    #[test]
    fn test_unnamed_config_block_construction() {
        let block = ConfigBlock {
            name: None,
            body: "cpp.defaults({})".to_string(),
            line: 1,
        };
        assert!(block.name.is_none());
    }

    #[test]
    fn test_cookfile_with_config_blocks() {
        let cookfile = Cookfile {
            config_blocks: vec![
                ConfigBlock { name: None,                    body: "base".into(), line: 1 },
                ConfigBlock { name: Some("release".into()),  body: "rel".into(),  line: 4 },
            ],
            recipes: vec![],
            chores: vec![],
            uses: vec![],
            imports: vec![],
            register_blocks: vec![],
            top_level_module_calls: vec![],
        };
        assert_eq!(cookfile.config_blocks.len(), 2);
        assert!(cookfile.config_blocks[0].name.is_none());
        assert_eq!(cookfile.config_blocks[1].name.as_deref(), Some("release"));
    }

    #[test]
    fn test_register_block_construction() {
        let block = RegisterBlock {
            body: "cook_cc.bin(\"game\", { sources = { \"src/main.c\" } })".to_string(),
            line: 3,
        };
        assert_eq!(block.line, 3);
        assert!(block.body.contains("cook_cc.bin"));
    }

    #[test]
    fn test_top_level_module_call_construction() {
        let call = TopLevelModuleCall {
            code: "cook_cc.bin(\"game\", { sources = { \"src/main.c\" } })".to_string(),
            line: 3,
        };
        assert_eq!(call.line, 3);
        assert!(call.code.contains("cook_cc.bin"));
    }

    // ── COOK-36: ChoreParam AST scaffold ─────────────────────────────

    #[test]
    fn chore_carries_empty_params_by_default() {
        let chore = Chore {
            name: "clean".to_string(),
            params: vec![],
            deps: vec![],
            steps: vec![],
            line: 1,
        };
        assert!(chore.params.is_empty());
    }

    #[test]
    fn chore_param_variants_construct() {
        let p_req = ChoreParam::Required { name: "target".into(), line: 1, col: 13 };
        let p_def_str = ChoreParam::DefaultedString {
            name: "host".into(), default: "prod".into(), line: 1, col: 20,
        };
        let p_def_lua = ChoreParam::DefaultedLua {
            name: "version".into(), default_lua: "cook.git.head_tag() or \"v0\"".into(),
            line: 1, col: 27,
        };
        let p_var_plus = ChoreParam::VariadicPlus { name: "FILES".into(), line: 1, col: 36 };
        let p_var_star = ChoreParam::VariadicStar { name: "EXTRAS".into(), line: 1, col: 44 };
        for p in [p_req, p_def_str, p_def_lua, p_var_plus, p_var_star] {
            let _ = p.clone();
        }
    }

    #[test]
    fn test_cookfile_with_register_blocks_and_top_level_calls() {
        let cookfile = Cookfile {
            config_blocks: vec![],
            recipes: vec![],
            chores: vec![],
            uses: vec![],
            imports: vec![],
            register_blocks: vec![
                RegisterBlock { body: "a()".into(), line: 1 },
                RegisterBlock { body: "b()".into(), line: 5 },
            ],
            top_level_module_calls: vec![
                TopLevelModuleCall { code: "cpp.bin(\"x\")".into(), line: 3 },
            ],
        };
        assert_eq!(cookfile.register_blocks.len(), 2);
        assert_eq!(cookfile.top_level_module_calls.len(), 1);
        assert_eq!(cookfile.top_level_module_calls[0].line, 3);
    }
}
