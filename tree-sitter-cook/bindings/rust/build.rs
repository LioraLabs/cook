fn main() {
    let src_dir = std::path::Path::new("src");

    let mut c_config = cc::Build::new();
    c_config.std("c11").include(src_dir);

    // `scanner.c` is the hand-written external scanner (brace balancing,
    // indentation-delimited body termination); `parser.c` is generated from
    // `grammar.js` by `tree-sitter generate`. Both are required.
    for path in ["parser.c", "scanner.c"] {
        let path = src_dir.join(path);
        c_config.file(&path);
        println!("cargo:rerun-if-changed={}", path.to_str().unwrap());
    }

    c_config.compile("tree-sitter-cook");
}
