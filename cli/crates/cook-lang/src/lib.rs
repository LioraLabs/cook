pub mod ast;
pub mod lexer;
pub(crate) mod cook_line;
pub(crate) mod lua_block;
pub(crate) mod recipe;

use ast::*;
use lexer::*;
use recipe::{parse_config_block, parse_recipe};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("{0}")]
    Lex(#[from] LexError),
    #[error("line {line}: {message}")]
    Parse { line: usize, message: String },
}

pub fn parse(source: &str) -> Result<Cookfile, ParseError> {
    let tokens = tokenize(source)?;
    let source_lines: Vec<&str> = source.lines().collect();
    let mut pos = 0;
    let mut recipes = Vec::new();
    let mut vars = Vec::new();
    let mut configs = std::collections::BTreeMap::new();
    let mut uses = Vec::new();
    let mut imports = Vec::new();
    let mut seen_recipe = false;

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            Token::VarDecl { name, value } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "variable declarations must appear before recipes".to_string(),
                    });
                }
                vars.push((name.clone(), value.clone()));
                pos += 1;
            }
            Token::ConfigHeader { name } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "config blocks must appear before recipes".to_string(),
                    });
                }
                let config_name = name.clone();
                pos += 1;
                let (config_vars, new_pos) = parse_config_block(&tokens, pos, tok.line)?;
                configs.insert(config_name, config_vars);
                pos = new_pos;
            }
            Token::RecipeHeader { name, deps } => {
                seen_recipe = true;
                let recipe_line = tok.line;
                let name = name.clone();
                let deps = deps.clone();
                pos += 1;
                let (recipe, new_pos) =
                    parse_recipe(name, deps, recipe_line, &tokens, pos, &source_lines)?;
                recipes.push(recipe);
                pos = new_pos;
            }
            Token::RecipeEnd => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "unexpected 'end' outside of a recipe or config block".to_string(),
                });
            }
            Token::Content(_) => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "unexpected content outside of a recipe".to_string(),
                });
            }
            Token::LuaLine(_) | Token::LuaBlockOpen => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "unexpected content outside of a recipe".to_string(),
                });
            }
            Token::UseDecl { name } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "use statements must appear before recipes".to_string(),
                    });
                }
                uses.push(ast::UseStatement {
                    module_name: name.clone(),
                    line: tok.line,
                });
                pos += 1;
            }
            Token::ImportDecl { name, path } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "import declarations must appear before recipes".to_string(),
                    });
                }
                if imports.iter().any(|i: &ast::ImportDecl| i.name == *name) {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: format!("duplicate import name '{}'", name),
                    });
                }
                imports.push(ast::ImportDecl {
                    name: name.clone(),
                    path: path.clone(),
                    line: tok.line,
                });
                pos += 1;
            }
        }
    }

    Ok(Cookfile { vars, configs, recipes, uses, imports })
}

#[cfg(test)]
mod tests;
