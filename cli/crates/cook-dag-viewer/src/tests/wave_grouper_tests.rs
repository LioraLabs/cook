use super::*;

#[test]
fn test_no_deps_single_wave() {
    let explicit = BTreeMap::new();
    let inferred = BTreeMap::new();
    let recipes = BTreeSet::from(["a".into(), "b".into(), "c".into()]);
    let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
    assert_eq!(waves.len(), 1);
    assert_eq!(waves[0].recipes.len(), 3);
}

#[test]
fn test_explicit_dep_creates_wave_boundary() {
    let mut explicit = BTreeMap::new();
    explicit.insert("run".to_string(), vec!["app".to_string()]);
    let inferred = BTreeMap::new();
    let recipes = BTreeSet::from(["app".into(), "run".into()]);
    let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
    assert_eq!(waves.len(), 2);
    assert!(waves[0].recipes.contains(&"app".to_string()));
    assert!(waves[1].recipes.contains(&"run".to_string()));
}

#[test]
fn test_inferred_dep_same_wave() {
    let explicit = BTreeMap::new();
    let mut inferred = BTreeMap::new();
    inferred.insert(
        "app".to_string(),
        vec!["libmath".to_string(), "libstr".to_string()],
        );
        let recipes = BTreeSet::from(["libmath".into(), "libstr".into(), "app".into()]);
    let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
    assert_eq!(waves.len(), 1);
    let app_pos = waves[0].recipes.iter().position(|r| r == "app").unwrap();
    let math_pos = waves[0]
        .recipes
        .iter()
        .position(|r| r == "libmath")
        .unwrap();
    let str_pos = waves[0]
        .recipes
        .iter()
        .position(|r| r == "libstr")
            .unwrap();
        assert!(math_pos < app_pos);
        assert!(str_pos < app_pos);
    }

    #[test]
    fn test_transitive_inferred_deps_collapse() {
        let explicit = BTreeMap::new();
        let mut inferred = BTreeMap::new();
        inferred.insert("core".to_string(), vec!["protos".to_string()]);
    inferred.insert("server".to_string(), vec!["core".to_string()]);
    let recipes = BTreeSet::from(["protos".into(), "core".into(), "server".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 1);
        let order: Vec<&str> = waves[0].recipes.iter().map(|s| s.as_str()).collect();
        assert!(
            order.iter().position(|&r| r == "protos").unwrap()
            < order.iter().position(|&r| r == "core").unwrap()
    );
    assert!(
        order.iter().position(|&r| r == "core").unwrap()
            < order.iter().position(|&r| r == "server").unwrap()
        );
    }

    #[test]
    fn test_mixed_explicit_and_inferred() {
        // libmath, libstr, app uses {libmath} {libstr}, run: app
        let mut explicit = BTreeMap::new();
        explicit.insert("run".to_string(), vec!["app".to_string()]);
    let mut inferred = BTreeMap::new();
    inferred.insert(
        "app".to_string(),
        vec!["libmath".to_string(), "libstr".to_string()],
        );
        let recipes = BTreeSet::from([
            "libmath".into(),
        "libstr".into(),
            "app".into(),
        "run".into(),
    ]);
    let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
    assert_eq!(waves.len(), 2);
    assert_eq!(waves[0].recipes.len(), 3); // libmath, libstr, app
    assert_eq!(waves[1].recipes, vec!["run".to_string()]);
}

#[test]
fn test_inferred_cycle_detected() {
    let explicit = BTreeMap::new();
    let mut inferred = BTreeMap::new();
    inferred.insert("a".to_string(), vec!["b".to_string()]);
    inferred.insert("b".to_string(), vec!["a".to_string()]);
    let recipes = BTreeSet::from(["a".into(), "b".into()]);
    let result = compute_waves(&explicit, &inferred, &recipes);
    assert!(result.is_err());
}

#[test]
fn test_inferred_respects_wave_boundaries() {
    // setup has no deps, libmath depends explicitly on setup,
    // app uses {libmath} (inferred)
    // Expected: wave 1 = setup, wave 2 = libmath + app
    let mut explicit = BTreeMap::new();
    explicit.insert("libmath".to_string(), vec!["setup".to_string()]);
    let mut inferred = BTreeMap::new();
    inferred.insert("app".to_string(), vec!["libmath".to_string()]);
    let recipes = BTreeSet::from(["setup".into(), "libmath".into(), "app".into()]);
    let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
    assert_eq!(waves.len(), 2);
    assert!(waves[0].recipes.contains(&"setup".to_string()));
    assert!(waves[1].recipes.contains(&"libmath".to_string()));
    assert!(waves[1].recipes.contains(&"app".to_string()));
}
