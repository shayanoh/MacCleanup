use std::io::ErrorKind;
use std::path::Path;

/// macOS blocks reads of `~/Library/Safari` until the terminal app has Full
/// Disk Access, so a permission error there means access is missing. A
/// missing folder means we cannot tell, which we treat as "not missing".
pub fn full_disk_access_missing(home: &Path) -> bool {
    match std::fs::read_dir(home.join("Library/Safari")) {
        Ok(_) => false,
        Err(e) => e.kind() == ErrorKind::PermissionDenied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_safari_dir_means_not_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!full_disk_access_missing(dir.path()));
    }

    #[test]
    fn readable_safari_dir_means_not_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Library/Safari")).unwrap();
        assert!(!full_disk_access_missing(dir.path()));
    }
}
