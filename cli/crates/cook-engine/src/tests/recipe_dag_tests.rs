use super::*;

fn edges(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(name, deps)| {
            (
                name.to_string(),
                deps.iter().map(|d| d.to_string()).collect(),
            )
        })
        .collect()
}

#[test]
fn test_single_recipe_ready_immediately() {
    let dep_edges = edges(&[("build", &[])]);
    let mut dag = RecipeDag::new(&dep_edges);
    let ready = dag.pop_ready();
    assert_eq!(ready, vec!["build"]);
    assert!(dag.pop_ready().is_empty());
}

#[test]
fn test_linear_chain() {
    let dep_edges = edges(&[("a", &["b"]), ("b", &[])]);
    let mut dag = RecipeDag::new(&dep_edges);

    let wave1 = dag.pop_ready();
    assert_eq!(wave1, vec!["b"]);

    dag.mark_done(&wave1);
    let wave2 = dag.pop_ready();
    assert_eq!(wave2, vec!["a"]);

    dag.mark_done(&wave2);
    assert!(dag.pop_ready().is_empty());
}

#[test]
fn test_diamond_two_middle_recipes_in_same_wave() {
    let dep_edges = edges(&[
        ("a", &["b", "c"]),
        ("b", &["d"]),
        ("c", &["d"]),
        ("d", &[]),
    ]);
    let mut dag = RecipeDag::new(&dep_edges);

    let wave1 = dag.pop_ready();
    assert_eq!(wave1, vec!["d"]);

    dag.mark_done(&wave1);
    let mut wave2 = dag.pop_ready();
    wave2.sort();
    assert_eq!(wave2, vec!["b", "c"]);

    dag.mark_done(&wave2);
    let wave3 = dag.pop_ready();
    assert_eq!(wave3, vec!["a"]);

    dag.mark_done(&wave3);
    assert!(dag.pop_ready().is_empty());
}

#[test]
fn test_all_independent_single_wave() {
    let dep_edges = edges(&[("a", &[]), ("b", &[]), ("c", &[])]);
    let mut dag = RecipeDag::new(&dep_edges);
    let mut wave = dag.pop_ready();
    wave.sort();
    assert_eq!(wave, vec!["a", "b", "c"]);

    dag.mark_done(&wave);
    assert!(dag.pop_ready().is_empty());
}

#[test]
fn test_empty_dag() {
    let dep_edges = edges(&[]);
    let mut dag = RecipeDag::new(&dep_edges);
    assert!(dag.pop_ready().is_empty());
}

#[test]
fn test_mark_done_decrements_dependents() {
    let dep_edges = edges(&[("a", &["b", "c"]), ("b", &[]), ("c", &[])]);
    let mut dag = RecipeDag::new(&dep_edges);

    let wave1 = dag.pop_ready();
    assert_eq!(wave1.len(), 2);

    dag.mark_done(&["b".to_string()]);
    assert!(dag.pop_ready().is_empty());

    dag.mark_done(&["c".to_string()]);
    let wave2 = dag.pop_ready();
    assert_eq!(wave2, vec!["a"]);
}
