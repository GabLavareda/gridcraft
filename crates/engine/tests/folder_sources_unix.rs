//! Unix-specific coverage for the Linux/BSD/macOS folder source. Run on a native Unix host.
#![cfg(unix)]
use gridcraft_engine::Session;
use serde_json::json;
use std::{os::unix::fs::symlink, path::PathBuf};

struct Folder(PathBuf);
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn hidden_files_and_platform_paths_are_preserved_without_following_symlink_cycles() {
    let folder = Folder(std::env::temp_dir().join(format!("gridcraft-unix-source-{}", std::process::id())));
    std::fs::create_dir_all(folder.0.join("sub")).unwrap();
    std::fs::write(folder.0.join(".hidden"), "hidden").unwrap();
    std::fs::write(folder.0.join("sub").join("data.csv"), "a\n1\n").unwrap();
    symlink(&folder.0, folder.0.join("sub").join("cycle")).unwrap();
    symlink(folder.0.join(".hidden"), folder.0.join("alias")).unwrap();
    let path = gridcraft_mquery::quote_text(folder.0.to_str().unwrap());
    let mut s = Session::new();
    s.new_workbook();
    let out =
        s.run("data.query.preview", json!({"formula":format!("Table.SelectColumns(Folder.Files({path}), {{\"Name\", \"Folder Path\"}})")})).unwrap();
    assert_eq!(out["rows"].as_array().unwrap().len(), 2);
    assert!(out["rows"].as_array().unwrap().iter().all(|r| r[1].as_str().unwrap().ends_with('/')));
    let hidden = s.run("data.query.preview", json!({"formula":format!("Folder.Files({path}){{[Name=\".hidden\"]}}[Attributes][Hidden]")})).unwrap();
    assert_eq!(hidden["rows"], json!([[true]]));
}
