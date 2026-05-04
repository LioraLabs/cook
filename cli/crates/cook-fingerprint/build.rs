fn main() {
    let target = std::env::var("TARGET").expect("TARGET set by cargo for build scripts");
    println!("cargo:rustc-env=COOK_TARGET_TRIPLE={}", target);
    println!("cargo:rerun-if-env-changed=TARGET");
}
