use std::path::Path;

/// Workspace layout: this test lives in crates/handshaker-core/tests and must
/// scan every Rust source directory under the workspace so CJK user text stays
/// out of all crates (core, cli, application, ffi, test-support).
fn workspace_crate_src_dirs() -> Vec<std::path::PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")); // crates/handshaker-core
    let root = manifest.parent().and_then(|dir| dir.parent());
    let mut dirs = Vec::new();
    if let Some(root) = root {
        let crates_dir = root.join("crates");
        if let Ok(entries) = std::fs::read_dir(&crates_dir) {
            for entry in entries.flatten() {
                let src = entry.path().join("src");
                if src.is_dir() {
                    dirs.push(src);
                }
            }
        }
        let root_src = root.join("src");
        if root_src.is_dir() {
            dirs.push(root_src);
        }
    }
    if dirs.is_empty() {
        // Fallback: at least scan this crate.
        dirs.push(manifest.join("src"));
    }
    dirs
}

#[test]
fn rust_sources_do_not_embed_cjk_user_text() {
    for directory in workspace_crate_src_dirs() {
        scan_directory(directory.as_path());
    }
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
