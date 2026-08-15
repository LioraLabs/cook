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
        lower(&format!("recipe r\n    gather \"src/*.c\"{body}")),
    );
}

#[test]
fn data_member_requires_single_quotes_in_shell_bodies() {
    for reference in ["$<in>", "\"$<in>\""] {
        let source = format!(
            "probe rows\n    json {{ echo '[{{\"name\":\"auth\"}}]' }}\n\nrecipe r\n    gather rows\n    cook \"out/$<in.name>\" {{ echo {reference} > $<out> }}\n"
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
        "probe rows\n    json { echo '[{\"name\":\"auth\"}]' }\n\nrecipe r\n    gather rows\n    cook \"out/$<in.name>\" { echo '$<in>' > $<out> }\n",
    );

    // COOK-483 / CS-0234: the law binds DATA members. A bare gather source
    // naming a `files` declaration binds PATH members — a string, never
    // composite — so it quotes exactly as a glob gather's member does, which
    // is to say not at all if the author does not want to. The inline and the
    // named spelling of one file set must compile the same.
    lower("recipe r\n    gather \"src/*.test.ts\"\n    test { node --check $<in> }\n");
    lower(
        "files srcs\n    \"src/*.test.ts\"\n\nrecipe r\n    gather srcs\n    test { node --check $<in> }\n",
    );
    lower(
        "files srcs\n    \"src/*.c\"\n\nrecipe r\n    gather srcs \"include/*.h\"\n    cook \"build/$<in.stem>.o\" { cc -c $<in> -o $<out> }\n",
    );

    for body in [
        "cook \"out/$<in.name>\" {\necho ok # $<in>\n}",
        "cook \"out/$<in.name>\" { cat <<'EOF' > $<out>\n$<in>\nEOF\n}",
        "cook \"out/$<in.name>\" >{ return item.name }",
        "cook \"out/$<in.name>\" { echo $<in.name> $<rows:value> > $<out> }",
    ] {
        lower(&format!(
            "probe rows\n    json {{ echo '[{{\"name\":\"auth\"}}]' }}\n\nrecipe r\n    gather rows\n    {body}\n"
        ));
    }
}
