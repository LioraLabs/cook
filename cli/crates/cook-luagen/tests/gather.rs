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
