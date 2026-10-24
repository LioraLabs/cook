//! A glob output reaching outside the unit's working directory is refused at
//! registration, before anything runs (CS-0275): `//**` once walked the host
//! from `/` after the command ran.
//!
//! The command here fails, so even with the refusal gone no output is ever
//! resolved and nothing is walked; every run is killed after 20 s besides.
//! The absolute form points inside the temp dir, never at a real `/x`.

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn cook(root: &Path, cache: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cook"))
        .arg("build")
        .current_dir(root)
        .env("XDG_CACHE_HOME", cache)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run cook");
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("cook build still running after 20 s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

/// `<tmp>/a/proj` holding a Cookfile whose one step declares `output` and runs `command`.
/// `<tmp>` in `output` stands for the temp dir.
fn project(output: &str, command: &str) -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let output = output.replace("<tmp>", &tmp.path().display().to_string());
    let proj = tmp.path().join("a/proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("Cookfile"),
        format!("recipe build\n    cook {output:?} {{ {command} }}\n"),
    )
    .unwrap();
    (tmp, proj)
}

#[test]
fn a_glob_output_leaving_the_unit_is_refused_before_anything_runs() {
    for output in ["//**", "/**", "<tmp>/a/other/**", "../gen/**", "x/../../*"] {
        let (tmp, proj) = project(output, "touch ran && exit 1");
        let output = output.replace("<tmp>", &tmp.path().display().to_string());
        let out = cook(&proj, &tmp.path().join("cache"));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{output}: {err}");
        assert!(
            err.contains(&format!("output {output:?} (recipe build) is a glob reaching outside the unit's working directory")),
            "{output}: {err}"
        );
        assert!(!proj.join("ran").exists(), "{output}: the command ran");
    }
}

/// cook_pnpm anchors its outputs as `<root>/./pkg/dist/**`: an absolute glob
/// under the working dir is inside it, so it builds and then hits.
#[test]
fn an_absolute_glob_output_under_the_unit_builds_and_hits() {
    let (tmp, proj) = project("dist/**", "mkdir -p pkg/dist && touch pkg/dist/a");
    let abs = format!("{}/./pkg/dist/**", proj.display());
    let cookfile = std::fs::read_to_string(proj.join("Cookfile")).unwrap();
    std::fs::write(proj.join("Cookfile"), cookfile.replace("\"dist/**\"", &format!("{abs:?}"))).unwrap();
    let out = cook(&proj, &tmp.path().join("cache"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = cook(&proj, &tmp.path().join("cache"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("1 cached recipes"), "not a hit: {err}");
    assert!(proj.join("pkg/dist/a").is_file());
}
