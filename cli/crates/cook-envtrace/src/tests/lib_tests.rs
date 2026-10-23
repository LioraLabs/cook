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
    assert!(text.lines().any(|l| l == "UNSET_BY_TEST"), "{text}");
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
