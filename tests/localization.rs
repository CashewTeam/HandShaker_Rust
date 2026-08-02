use std::path::Path;

#[test]
fn rust_sources_do_not_embed_cjk_user_text() {
    scan_directory(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").as_path());
}

fn scan_directory(directory: &Path) {
    for entry in std::fs::read_dir(directory).expect("read source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            scan_directory(&path);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read source file");
        assert!(
            !source
                .chars()
                .any(|character| ('\u{3400}'..='\u{9fff}').contains(&character)),
            "CJK text must be stored in a language file: {}",
            path.display()
        );
    }
}
