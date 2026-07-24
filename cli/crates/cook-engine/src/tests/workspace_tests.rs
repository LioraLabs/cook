use super::*;

#[test]
fn register_workspace_for_test_includes_all_recipes_across_imports() {
    use std::collections::BTreeSet;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(root.join("Cookfile"), "\
import sub ./sub\n\
recipe build\n\
cook \"build/r.txt\" { echo > $<out> }\n\
").unwrap();

    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/Cookfile"), "\
recipe inner\n\
cook \"build/i.txt\" { echo > $<out> }\n\
recipe test_only\n\
test { true }\n\
").unwrap();

    let result = register_workspace_for_test(root).expect("must succeed");
    let names: BTreeSet<_> = result.keys().cloned().collect();
    assert!(names.contains("build"), "root recipe must be present");
    assert!(names.contains("sub.inner"), "imported recipe must be present");
    assert!(
        names.contains("sub.test_only"),
        "test_only is not referenced by any target but must still be registered; got: {names:?}"
    );
}

#[test]
fn register_workspace_for_test_root_only_no_imports() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("Cookfile"), "recipe alpha\nrecipe beta\n").unwrap();
    let result = register_workspace_for_test(root).expect("must succeed");
    assert!(result.contains_key("alpha"));
    assert!(result.contains_key("beta"));
}
