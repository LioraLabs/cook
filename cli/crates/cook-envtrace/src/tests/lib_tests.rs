use super::*;
use std::process::Command;

#[test]
fn records_names_of_set_and_unset_variables_but_never_values() {
    let dir = tempfile::tempdir().unwrap();
    let so = install(dir.path()).unwrap().unwrap();
    let out = dir.path().join("names");
    let st = Command::new("node")
        .args(["-e", "process.env.SET_BY_TEST; process.env.UNSET_BY_TEST"])
        .envs(env_for(&so, &out))
        .env("SET_BY_TEST", "s3cret-value")
        .status();
    let Ok(st) = st else { return eprintln!("skipping: node not found") };
    assert!(st.success());
    let text = std::fs::read_to_string(&out).unwrap();
    // `pid tid index long count NAME`: an unset name has no index, a set one its place in environ.
    let index = |name: &str| text.lines().find(|l| l.ends_with(&format!(" {name}"))).map(|l| l.split(' ').nth(2).unwrap().to_string());
    assert_eq!(index("UNSET_BY_TEST").as_deref(), Some("-"), "{text}");
    assert!(index("SET_BY_TEST").unwrap().parse::<usize>().is_ok(), "{text}");
    assert!(!text.contains("s3cret-value"), "{text}");
}

#[test]
fn a_record_that_cannot_be_written_leaves_the_callers_errno_alone() {
    let dir = tempfile::tempdir().unwrap();
    let so = install(dir.path()).unwrap().unwrap();
    let src = dir.path().join("probe.c");
    std::fs::write(
        &src,
        "#include <stdlib.h>\n#include <errno.h>\nint main(void){errno=0;getenv(\"X\");return errno;}\n",
    )
    .unwrap();
    let exe = dir.path().join("probe");
    let built = Command::new("cc").arg(&src).arg("-o").arg(&exe).status();
    match built {
        Ok(s) if s.success() => {}
        _ if std::env::var("COOK_AUDIT_E2E").as_deref() == Ok("require") => panic!("cc is required"),
        _ => return eprintln!("skipping: cc not usable"),
    }
    let st = Command::new(&exe)
        .envs(env_for(&so, &dir.path().join("no-such-dir/names")))
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(0), "getenv changed errno");
}

#[test]
fn a_path_ld_preload_would_split_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let spaced = dir.path().join("a b");
    std::fs::create_dir(&spaced).unwrap();
    let err = install(&spaced).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    assert!(err.to_string().contains("LD_PRELOAD cannot carry"), "{err}");
    assert!(std::fs::read_dir(&spaced).unwrap().next().is_none(), "nothing is written");
}

#[test]
fn a_value_of_256_bytes_or_more_is_flagged_long_and_never_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let so = install(dir.path()).unwrap().unwrap();
    let out = dir.path().join("names");
    let st = Command::new("node")
        .args(["-e", "process.env.SHORT_BY_TEST; process.env.LONG_BY_TEST; process.env.UNSET_BY_TEST"])
        .envs(env_for(&so, &out))
        .env("SHORT_BY_TEST", "x".repeat(255))
        .env("LONG_BY_TEST", "y".repeat(256))
        .status();
    let Ok(st) = st else { return eprintln!("skipping: node not found") };
    assert!(st.success());
    let text = std::fs::read_to_string(&out).unwrap();
    let long = |name: &str| text.lines().find(|l| l.ends_with(&format!(" {name}"))).map(|l| l.split(' ').nth(3).unwrap().to_string());
    assert_eq!(long("SHORT_BY_TEST").as_deref(), Some("0"), "{text}");
    assert_eq!(long("LONG_BY_TEST").as_deref(), Some("1"), "{text}");
    assert_eq!(long("UNSET_BY_TEST").as_deref(), Some("0"), "{text}");
    assert!(!text.contains("yyyy"), "{text}");
}

#[test]
fn env_for_carries_long_and_short_reference_values_no_prefix_scan_picks() {
    let vars = env_for(Path::new("/x/rec.so"), Path::new("/x/names"));
    let long: Vec<&str> = REFERENCES.iter().map(|(n, _)| *n).filter(|n| vars[*n].len() >= 256).collect();
    assert_eq!(long.len(), 2, "{long:?}");
    assert!(REFERENCES.iter().any(|(n, _)| vars[*n].len() < 256));
    // No uppercase, so a scan for COOK_, C or [A-Z_]+ picks none; the long ones start differently.
    assert!(REFERENCES.iter().all(|(n, _)| !n.chars().any(|c| c.is_ascii_uppercase())));
    assert_ne!(long[0].as_bytes()[0].is_ascii_alphabetic(), long[1].as_bytes()[0].is_ascii_alphabetic());
}
