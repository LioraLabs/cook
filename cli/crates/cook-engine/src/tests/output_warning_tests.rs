#[test]
fn output_warning_keeps_recipe_attribution() {
    let tmp = tempfile::tempdir().unwrap();
    let warnings = super::collect_output_glob_warnings_for_recipe(
        "assets",
        tmp.path(),
        &["dist/**".to_string(), "manifest.json".to_string()],
    );
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].pattern, "dist/**");
    assert_eq!(warnings[0].recipe, "assets");
}
