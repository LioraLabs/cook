#[derive(Debug, Clone, PartialEq)]
pub struct UseStatement {
    pub module_name: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub name: String,
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigBlock {
    pub name: Option<String>,
    pub body: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cookfile {
    pub config_blocks: Vec<ConfigBlock>,
    pub recipes: Vec<Recipe>,
    pub uses: Vec<UseStatement>,
    pub imports: Vec<ImportDecl>,
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
    Shell(String),
    LuaBlock(String),
    ShellBlock(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CookStep {
    pub outputs: Vec<String>,
    pub using_clause: Option<UsingClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlateStep {
    pub command: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TestStep {
    pub command: String,
    pub timeout: Option<u64>,
    pub should_fail: bool,
}

#[derive(Debug, Clone, PartialEq)]
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
                        outputs: vec!["build/obj/{stem}.o".to_string()],
                        using_clause: Some(UsingClause::Shell(
                            "gcc -c {in} -o {out}".to_string(),
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
            outputs: vec!["bin/app".to_string()],
            using_clause: None,
        };
        assert!(step.using_clause.is_none());
    }

    #[test]
    fn test_cook_step_lua_block() {
        let step = CookStep {
            outputs: vec!["build/obj/{stem}.o".to_string()],
            using_clause: Some(UsingClause::LuaBlock(
                "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
            )),
        };
        assert!(matches!(step.using_clause, Some(UsingClause::LuaBlock(_))));
    }

    #[test]
    fn test_plate_step() {
        let step = PlateStep {
            command: "./{out}".to_string(),
        };
        assert_eq!(step.command, "./{out}");
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
            uses: vec![UseStatement { module_name: "cpp".to_string(), line: 1 }],
            imports: vec![],
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
            uses: vec![],
            imports: vec![],
        };
        assert_eq!(cookfile.config_blocks.len(), 2);
        assert!(cookfile.config_blocks[0].name.is_none());
        assert_eq!(cookfile.config_blocks[1].name.as_deref(), Some("release"));
    }
}
