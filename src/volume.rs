use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

fn stat(path: &Path) -> io::Result<libc::statvfs> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    // SAFETY: `statvfs` is plain-old-data; the call fills it on success.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated string and `s` a valid out pointer.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(s)
}

/// Bytes available to this user on the volume that holds `path`.
pub fn free_space(path: &Path) -> io::Result<u64> {
    let s = stat(path)?;
    Ok((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
}

/// Total size in bytes of the volume that holds `path`.
pub fn total_space(path: &Path) -> io::Result<u64> {
    let s = stat(path)?;
    Ok((s.f_blocks as u64).saturating_mul(s.f_frsize as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn free_is_positive_and_not_more_than_total() {
        let tmp = std::env::temp_dir();
        let free = free_space(&tmp).unwrap();
        let total = total_space(&tmp).unwrap();
        assert!(free > 0);
        assert!(total >= free);
    }

    #[test]
    fn error_on_missing_path() {
        assert!(free_space(Path::new("/definitely/not/a/real/path")).is_err());
        assert!(total_space(Path::new("/definitely/not/a/real/path")).is_err());
    }
}
