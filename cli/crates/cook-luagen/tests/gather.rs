use cook_luagen::{dep_ref::extract_recipe_names, generate_checked};

fn lower(source: &str) -> String {
    let cookfile = cook_lang::parse(source).unwrap();
    let names = extract_recipe_names(&cookfile);
    generate_checked(&cookfile, &names).unwrap().0
}

#[test]
fn gather_is_the_existing_register_time_driver_not_a_unit() {
    let body = "\n    cook \"build/$<in.stem>.o\" { cp $<in> $<out> }\n";
    assert_eq!(
        lower(&format!("recipe r\n    gather \"src/*.c\"{body}")),
        lower(&format!("recipe r\n    ingredients \"src/*.c\"{body}")),
    );
}

#[test]
fn data_member_requires_single_quotes_in_shell_bodies() {
    for reference in ["$<in>", "\"$<in>\""] {
        let source = format!(
            "probe rows\n    json {{ echo '[{{\"name\":\"auth\"}}]' }}\n\nrecipe r\n    ingredients rows\n    cook \"out/$<in.name>\" {{ echo {reference} > $<out> }}\n"
        );
        let cookfile = cook_lang::parse(&source).unwrap();
        let names = extract_recipe_names(&cookfile);
        let error = generate_checked(&cookfile, &names).unwrap_err().to_string();
        assert!(
            error.contains("$<in>") && error.contains("single quotes") && error.contains("line 6"),
            "unexpected diagnostic: {error}"
        );
    }

    lower(
        "probe rows\n    json { echo '[{\"name\":\"auth\"}]' }\n\nrecipe r\n    ingredients rows\n    cook \"out/$<in.name>\" { echo '$<in>' > $<out> }\n",
    );

    for body in [
        "cook \"out/$<in.name>\" {\necho ok # $<in>\n}",
        "cook \"out/$<in.name>\" { cat <<'EOF' > $<out>\n$<in>\nEOF\n}",
        "cook \"out/$<in.name>\" >{ return item.name }",
        "cook \"out/$<in.name>\" { echo $<in.name> $<rows:value> > $<out> }",
    ] {
        lower(&format!(
            "probe rows\n    json {{ echo '[{{\"name\":\"auth\"}}]' }}\n\nrecipe r\n    ingredients rows\n    {body}\n"
        ));
    }
}
