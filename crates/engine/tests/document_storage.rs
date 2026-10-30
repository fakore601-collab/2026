use std::fs;
use std::path::PathBuf;
use wordcraft_doc::Document;
use wordcraft_engine::io::{open_path_with_backup_recovery, save_path_atomic_with_backup};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("wordcraft_test_{}_{}", name, std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn test_atomic_save_and_recovery() {
    let temp = TempDir::new("atomic_save");
    let doc_path = temp.0.join("test_doc.docx");
    let bak_path = temp.0.join("test_doc.docx.bak");

    let doc_v1 = Document::from_text("Version 1 content");
    save_path_atomic_with_backup(&doc_path, &doc_v1, None, &Default::default()).unwrap();

    assert!(doc_path.is_file(), "Primary file must exist after first save");
    assert!(!bak_path.is_file(), "Backup file should not exist on first save");

    let (loaded_v1, is_bak) = open_path_with_backup_recovery(&doc_path, None).unwrap();
    assert!(!is_bak, "Should load primary file, not backup");
    assert_eq!(loaded_v1.plain_text(wordcraft_doc::StoryRef::Body), "Version 1 content");

    // Second save -> creates .bak
    let doc_v2 = Document::from_text("Version 2 updated content");
    save_path_atomic_with_backup(&doc_path, &doc_v2, None, &Default::default()).unwrap();

    assert!(doc_path.is_file(), "Primary file must exist");
    assert!(bak_path.is_file(), "Backup file must be created on subsequent save");

    let (loaded_v2, is_bak) = open_path_with_backup_recovery(&doc_path, None).unwrap();
    assert!(!is_bak, "Should load primary file");
    assert_eq!(loaded_v2.plain_text(wordcraft_doc::StoryRef::Body), "Version 2 updated content");
}

#[test]
fn test_corrupted_primary_falls_back_to_backup() {
    let temp = TempDir::new("corrupted_fallback");
    let doc_path = temp.0.join("damaged_doc.docx");
    let bak_path = temp.0.join("damaged_doc.docx.bak");

    // Save v1
    let doc_v1 = Document::from_text("Resilient content from backup");
    save_path_atomic_with_backup(&doc_path, &doc_v1, None, &Default::default()).unwrap();

    // Save v2
    let doc_v2 = Document::from_text("Version 2 will be damaged");
    save_path_atomic_with_backup(&doc_path, &doc_v2, None, &Default::default()).unwrap();
    assert!(bak_path.is_file());

    // Corrupt primary file by overwriting with arbitrary garbage bytes
    fs::write(&doc_path, b"CORRUPTED_GARBAGE_DATA_NOT_A_VALID_DOCX").unwrap();

    // Open should automatically fall back to .bak!
    let (recovered_doc, is_bak) = open_path_with_backup_recovery(&doc_path, None).unwrap();
    assert!(is_bak, "Recovery flag must be true when loading from backup");
    assert_eq!(recovered_doc.plain_text(wordcraft_doc::StoryRef::Body), "Resilient content from backup");
}

#[test]
fn test_corrupted_file_without_backup_fails_gracefully() {
    let temp = TempDir::new("corrupted_no_backup");
    let doc_path = temp.0.join("broken.docx");

    // Write corrupted bytes directly
    fs::write(&doc_path, b"TRUNCATED_BAD_BYTES").unwrap();

    // Attempt to open -> returns Err gracefully without panicking
    let result = open_path_with_backup_recovery(&doc_path, None);
    assert!(result.is_err(), "Must return Err for corrupted file without backup");
}
