use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::safety::{Policy, Safety};

#[derive(Debug, Error)]
pub enum HoldError {
    #[error("{0:?} is protected and cannot be deleted")]
    Protected(PathBuf),
    #[error("{0:?} is on a different volume than the ToBeDeleted folder")]
    CrossVolume(PathBuf),
    #[error("{0:?} already exists, so the item cannot be restored there")]
    Conflict(PathBuf),
    #[error("no held item with id {0:?}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Paths are stored as raw bytes so non-UTF-8 names survive the manifest.
mod path_bytes {
    use super::*;

    // serde's `with` passes `&PathBuf` (the field type), so the signature must match.
    #[allow(clippy::ptr_arg)]
    pub fn serialize<S: serde::Serializer>(p: &PathBuf, s: S) -> Result<S::Ok, S::Error> {
        p.as_os_str().as_bytes().serialize(s)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        let bytes = Vec::<u8>::deserialize(d)?;
        Ok(PathBuf::from(OsString::from_vec(bytes)))
    }
}

/// One soft-deleted item. The payload lives at `<dir>/items/<id>/payload`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldItem {
    pub id: String,
    #[serde(with = "path_bytes")]
    pub original: PathBuf,
    /// Display name only (lossy); the real file is always `payload`.
    pub held_name: String,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub deleted_at: u64,
}

/// The soft-delete holding folder (default `~/ToBeDeleted`).
pub struct Holding {
    dir: PathBuf,
    lock: Mutex<()>,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos}-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn map_rename_error(e: io::Error, path: &Path) -> HoldError {
    if e.raw_os_error() == Some(libc::EXDEV) {
        HoldError::CrossVolume(path.to_path_buf())
    } else {
        HoldError::Io(e)
    }
}

impl Holding {
    pub fn open(dir: PathBuf) -> io::Result<Holding> {
        fs::create_dir_all(dir.join("items"))?;
        let holding = Holding {
            dir,
            lock: Mutex::new(()),
        };
        if !holding.manifest_path().exists() {
            holding.write_manifest(&[])?;
        }
        Ok(holding)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.json")
    }

    fn read_manifest(&self) -> io::Result<Vec<HeldItem>> {
        match fs::read(self.manifest_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn write_manifest(&self, items: &[HeldItem]) -> io::Result<()> {
        let tmp = self.dir.join("manifest.json.tmp");
        let bytes = serde_json::to_vec_pretty(items).map_err(io::Error::other)?;
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, self.manifest_path())
    }

    /// Ids are digits and dashes only, so they can never escape `items/`.
    fn item_dir(&self, id: &str) -> Result<PathBuf, HoldError> {
        let valid = !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit() || b == b'-');
        if !valid {
            return Err(HoldError::NotFound(id.to_string()));
        }
        Ok(self.dir.join("items").join(id))
    }

    /// Move `path` into the holding folder. Refuses protected paths, the
    /// holding folder itself, and anything on another volume.
    pub fn move_in(&self, policy: &Policy, path: &Path, size: u64) -> Result<HeldItem, HoldError> {
        if policy.classify_real(path) == Safety::Protected {
            return Err(HoldError::Protected(path.to_path_buf()));
        }
        let name = path
            .file_name()
            .ok_or_else(|| HoldError::Protected(path.to_path_buf()))?;
        // Defense in depth: never move the holding folder or anything inside it.
        let real = path
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p.join(name));
        if let (Some(real), Ok(held)) = (real, self.dir.canonicalize())
            && (real.starts_with(&held) || held.starts_with(&real))
        {
            return Err(HoldError::Protected(path.to_path_buf()));
        }
        fs::symlink_metadata(path)?;

        let id = new_id();
        let item_dir = self.item_dir(&id)?;
        fs::create_dir(&item_dir)?;
        let payload = item_dir.join("payload");
        if let Err(e) = fs::rename(path, &payload) {
            let _ = fs::remove_dir(&item_dir);
            return Err(map_rename_error(e, path));
        }
        let item = HeldItem {
            id,
            original: path.to_path_buf(),
            held_name: name.to_string_lossy().into_owned(),
            size,
            deleted_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        };
        let _g = self.guard();
        let result = self.read_manifest().and_then(|mut items| {
            items.push(item.clone());
            self.write_manifest(&items)
        });
        if let Err(e) = result {
            // Put the file back rather than leave it untracked.
            let _ = fs::rename(&payload, path);
            let _ = fs::remove_dir(&item_dir);
            return Err(HoldError::Io(e));
        }
        Ok(item)
    }

    pub fn list(&self) -> io::Result<Vec<HeldItem>> {
        let _g = self.guard();
        self.read_manifest()
    }

    pub fn total_size(&self) -> io::Result<u64> {
        Ok(self.list()?.iter().map(|i| i.size).sum())
    }

    /// Put an item back where it came from.
    pub fn restore(&self, id: &str) -> Result<PathBuf, HoldError> {
        let item_dir = self.item_dir(id)?;
        let _g = self.guard();
        let mut items = self.read_manifest()?;
        let pos = items
            .iter()
            .position(|i| i.id == id)
            .ok_or_else(|| HoldError::NotFound(id.to_string()))?;
        let original = items[pos].original.clone();
        if fs::symlink_metadata(&original).is_ok() {
            return Err(HoldError::Conflict(original));
        }
        if let Some(parent) = original.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(item_dir.join("payload"), &original)
            .map_err(|e| map_rename_error(e, &original))?;
        let _ = fs::remove_dir(&item_dir);
        items.remove(pos);
        self.write_manifest(&items)?;
        Ok(original)
    }

    /// Permanently delete one held item. Returns the bytes it held.
    pub fn delete_forever(&self, id: &str) -> Result<u64, HoldError> {
        let item_dir = self.item_dir(id)?;
        let _g = self.guard();
        let mut items = self.read_manifest()?;
        let pos = items
            .iter()
            .position(|i| i.id == id)
            .ok_or_else(|| HoldError::NotFound(id.to_string()))?;
        match fs::remove_dir_all(&item_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(HoldError::Io(e)),
        }
        let size = items.remove(pos).size;
        self.write_manifest(&items)?;
        Ok(size)
    }

    /// Permanently delete everything held. Returns the bytes removed.
    pub fn empty(&self) -> Result<u64, HoldError> {
        let ids: Vec<String> = self.list()?.into_iter().map(|i| i.id).collect();
        let mut total = 0u64;
        for id in ids {
            total = total.saturating_add(self.delete_forever(&id)?);
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::Policy;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    struct Sandbox {
        _dir: TempDir,
        home: PathBuf,
        policy: Policy,
        holding: Holding,
    }

    fn sandbox() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        fs::create_dir_all(&home).unwrap();
        let holding_dir = home.join("ToBeDeleted");
        let holding = Holding::open(holding_dir.clone()).unwrap();
        let policy = Policy::new(&home, &holding_dir);
        Sandbox {
            _dir: dir,
            home,
            policy,
            holding,
        }
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn move_and_list() {
        let s = sandbox();
        let file = s.home.join("proj/a.txt");
        write(&file, "hello");
        let item = s.holding.move_in(&s.policy, &file, 5).unwrap();
        assert!(!file.exists());
        let items = s.holding.list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].original, file);
        assert_eq!(items[0].id, item.id);
        assert_eq!(s.holding.total_size().unwrap(), 5);
    }

    #[test]
    fn protected_is_refused() {
        let s = sandbox();
        let docs = s.home.join("Documents");
        fs::create_dir_all(&docs).unwrap();
        let err = s.holding.move_in(&s.policy, &docs, 0).unwrap_err();
        assert!(matches!(err, HoldError::Protected(_)), "{err:?}");
        assert!(docs.exists());
        assert!(s.holding.list().unwrap().is_empty());
    }

    #[test]
    fn cannot_move_holding_into_itself() {
        let s = sandbox();
        let dir = s.holding.dir().to_path_buf();
        let err = s.holding.move_in(&s.policy, &dir, 0).unwrap_err();
        assert!(matches!(err, HoldError::Protected(_)), "{err:?}");
        assert!(dir.exists());
    }

    #[test]
    fn missing_path_is_an_io_error() {
        let s = sandbox();
        fs::create_dir_all(s.home.join("proj")).unwrap();
        let err = s
            .holding
            .move_in(&s.policy, &s.home.join("proj/nope"), 0)
            .unwrap_err();
        assert!(matches!(err, HoldError::Io(_)), "{err:?}");
    }

    #[test]
    fn symlink_moves_link_not_target() {
        let s = sandbox();
        let real = s.home.join("proj/real");
        write(&real, "data");
        let link = s.home.join("proj/l");
        symlink(&real, &link).unwrap();
        s.holding.move_in(&s.policy, &link, 4).unwrap();
        assert!(fs::symlink_metadata(&link).is_err(), "link should be gone");
        assert_eq!(fs::read_to_string(&real).unwrap(), "data");
    }

    #[test]
    fn restore_roundtrip() {
        let s = sandbox();
        let file = s.home.join("proj/a.txt");
        write(&file, "hello");
        let item = s.holding.move_in(&s.policy, &file, 5).unwrap();
        let back = s.holding.restore(&item.id).unwrap();
        assert_eq!(back, file);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
        assert!(s.holding.list().unwrap().is_empty());
    }

    #[test]
    fn directory_roundtrip() {
        let s = sandbox();
        let dir = s.home.join("proj/build");
        write(&dir.join("x/y.txt"), "deep");
        let item = s.holding.move_in(&s.policy, &dir, 4).unwrap();
        assert!(!dir.exists());
        s.holding.restore(&item.id).unwrap();
        assert_eq!(fs::read_to_string(dir.join("x/y.txt")).unwrap(), "deep");
    }

    #[test]
    fn restore_conflict() {
        let s = sandbox();
        let file = s.home.join("proj/a.txt");
        write(&file, "old");
        let item = s.holding.move_in(&s.policy, &file, 3).unwrap();
        write(&file, "new");
        let err = s.holding.restore(&item.id).unwrap_err();
        assert!(matches!(err, HoldError::Conflict(_)), "{err:?}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(s.holding.list().unwrap().len(), 1);
    }

    #[test]
    fn restore_recreates_missing_parent() {
        let s = sandbox();
        let file = s.home.join("proj/sub/a.txt");
        write(&file, "hello");
        let item = s.holding.move_in(&s.policy, &file, 5).unwrap();
        fs::remove_dir_all(s.home.join("proj")).unwrap();
        s.holding.restore(&item.id).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[test]
    fn same_name_items_do_not_collide() {
        let s = sandbox();
        let x = s.home.join("x/a.txt");
        let y = s.home.join("y/a.txt");
        write(&x, "from x");
        write(&y, "from y");
        let ix = s.holding.move_in(&s.policy, &x, 6).unwrap();
        let iy = s.holding.move_in(&s.policy, &y, 6).unwrap();
        assert_ne!(ix.id, iy.id);
        s.holding.restore(&iy.id).unwrap();
        s.holding.restore(&ix.id).unwrap();
        assert_eq!(fs::read_to_string(&x).unwrap(), "from x");
        assert_eq!(fs::read_to_string(&y).unwrap(), "from y");
    }

    #[test]
    fn delete_forever_and_empty() {
        let s = sandbox();
        let a = s.home.join("p/a.txt");
        let b = s.home.join("p/b.txt");
        let keep = s.home.join("p/keep.txt");
        write(&a, "aaaa");
        write(&b, "bb");
        write(&keep, "untouched");
        let ia = s.holding.move_in(&s.policy, &a, 4).unwrap();
        s.holding.move_in(&s.policy, &b, 2).unwrap();

        assert_eq!(s.holding.delete_forever(&ia.id).unwrap(), 4);
        assert_eq!(s.holding.list().unwrap().len(), 1);
        assert_eq!(s.holding.empty().unwrap(), 2);
        assert!(s.holding.list().unwrap().is_empty());
        assert_eq!(s.holding.total_size().unwrap(), 0);
        assert_eq!(fs::read_to_string(&keep).unwrap(), "untouched");
        let leftovers = fs::read_dir(s.holding.dir().join("items")).unwrap().count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn unknown_id_is_not_found() {
        let s = sandbox();
        for id in ["nope", "../escape", "", "a/b"] {
            assert!(matches!(s.holding.restore(id), Err(HoldError::NotFound(_))));
            assert!(matches!(
                s.holding.delete_forever(id),
                Err(HoldError::NotFound(_))
            ));
        }
    }

    #[test]
    fn cross_volume_error_mapping() {
        let p = Path::new("/a/b");
        let e = io::Error::from_raw_os_error(libc::EXDEV);
        assert!(matches!(map_rename_error(e, p), HoldError::CrossVolume(_)));
        let e = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(matches!(map_rename_error(e, p), HoldError::Io(_)));
    }

    #[test]
    fn manifest_survives_reopen() {
        let s = sandbox();
        let file = s.home.join("proj/a.txt");
        write(&file, "hello");
        s.holding.move_in(&s.policy, &file, 5).unwrap();
        let again = Holding::open(s.holding.dir().to_path_buf()).unwrap();
        let items = again.list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].original, file);
    }

    #[test]
    fn non_utf8_original_path_roundtrips() {
        use std::os::unix::ffi::OsStrExt;
        let s = sandbox();
        let name = std::ffi::OsStr::from_bytes(&[0x66, 0x80, 0x6f]);
        let file = s.home.join("proj").join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        if fs::write(&file, b"x").is_err() {
            eprintln!("skipping: filesystem rejects non-UTF-8 names");
            return;
        }
        let item = s.holding.move_in(&s.policy, &file, 1).unwrap();
        let again = Holding::open(s.holding.dir().to_path_buf()).unwrap();
        assert_eq!(again.list().unwrap()[0].original, file);
        again.restore(&item.id).unwrap();
        assert!(file.exists());
    }
}
