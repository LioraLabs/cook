//! A recipe cache that cannot be saved is reported once for the invocation,
//! however many recipes share the failure.

use std::process::Command;

use tempfile::TempDir;

const COOKFILE: &str = "recipe a\n    cook \"a.out\" { echo a > a.out }\n\n\
recipe b\n    cook \"b.out\" { echo b > b.out }\n\n\
recipe top\n    cook \"t.out\" { cat $<a> $<b> > t.out }\n";

#[test]
fn unsaveable_recipe_caches_are_reported_once() {
    let tmp = TempDir::new().unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_cook"))
            .arg("top")
            .current_dir(tmp.path())
            .env("XDG_CACHE_HOME", tmp.path().join("xdg"))
            .output()
            .expect("run cook")
    };
    std::fs::write(tmp.path().join("Cookfile"), COOKFILE).unwrap();
    assert!(run().status.success());
    // A directory where the index file belongs: the save fails whoever runs it, root included.
    for recipe in ["a", "b", "top"] {
        let index = tmp.path().join(format!(".cook/cache/{recipe}.idx"));
        std::fs::remove_file(&index).unwrap();
        std::fs::create_dir(&index).unwrap();
    }
    std::fs::remove_file(tmp.path().join("t.out")).unwrap();
    let out = run();
    let err = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<_> = err.lines().filter(|l| l.contains("recipe cache not persisted")).collect();
    assert_eq!(lines.len(), 1, "one line for three recipes:\n{err}");
    assert!(lines[0].starts_with("cook: warning: recipe cache not persisted for 3 recipe(s)"), "{err}");
}
