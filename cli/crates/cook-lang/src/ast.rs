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
pub struct Cookfile {
    pub vars: Vec<(String, String)>,
    pub configs: std::collections::BTreeMap<String, Vec<(String, String)>>,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct CookStep {
    pub output_pattern: String,
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
    Lua { code: String, line: usize },
    LuaBlock { code: String, line: usize },
    Cook { step: CookStep, line: usize },
    Plate { step: PlateStep, line: usize },
    Test { step: TestStep, line: usize },
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
                        output_pattern: "build/obj/{stem}.o".to_string(),
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
            output_pattern: "bin/app".to_string(),
            using_clause: None,
        };
        assert!(step.using_clause.is_none());
    }

    #[test]
    fn test_cook_step_lua_block() {
        let step = CookStep {
            output_pattern: "build/obj/{stem}.o".to_string(),
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
    fn test_cookfile_with_vars_and_configs() {
        let cookfile = Cookfile {
            vars: vec![
                ("CC".to_string(), "gcc".to_string()),
                ("CFLAGS".to_string(), "-Wall".to_string()),
            ],
            configs: {
                let mut m = std::collections::BTreeMap::new();
                m.insert("debug".to_string(), vec![
                    ("CFLAGS".to_string(), "-g -O0 -Wall".to_string()),
                ]);
                m
            },
            recipes: vec![],
            uses: vec![],
            imports: vec![],
        };
        assert_eq!(cookfile.vars.len(), 2);
        assert_eq!(cookfile.configs.len(), 1);
        assert_eq!(cookfile.configs["debug"].len(), 1);
    }

    #[test]
    fn test_cookfile_with_uses() {
        let cookfile = Cookfile {
            vars: vec![],
            configs: std::collections::BTreeMap::new(),
            recipes: vec![],
            uses: vec![UseStatement { module_name: "cpp".to_string(), line: 1 }],
            imports: vec![],
        };
        assert_eq!(cookfile.uses.len(), 1);
        assert_eq!(cookfile.uses[0].module_name, "cpp");
    }
}
