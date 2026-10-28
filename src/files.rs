//! Files romi keeps for itself: writes that never leave a half-written file behind, caches of
//! values worked out from files, and folders in /tmp that clean up after themselves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Writes `contents` to `path` through a temporary file next to it, so a crash never leaves a
/// half-written file. Makes the folder if it isn't there.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// A file's size and modification time (in seconds), which together say whether it changed.
pub fn stamp(path: &Path) -> Option<(u64, u64)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some((md.len(), mtime))
}

/// Values worked out from files, like checksums, remembered with each file's stamp so a file is
/// only read again once it changes. Saved as lines of path, size, mtime and value.
pub struct StampCache<V> {
    path: PathBuf,
    known: HashMap<String, (u64, u64, V)>,
    dirty: bool,
}

/// How a `StampCache` value is written in its file.
pub trait CacheValue: Sized + Clone {
    fn parse(text: &str) -> Option<Self>;
    fn write(&self) -> String;
}

impl CacheValue for String {
    fn parse(text: &str) -> Option<Self> {
        Some(text.to_string())
    }

    fn write(&self) -> String {
        self.clone()
    }
}

/// CRC-32s, in hex.
impl CacheValue for u32 {
    fn parse(text: &str) -> Option<Self> {
        u32::from_str_radix(text, 16).ok()
    }

    fn write(&self) -> String {
        format!("{self:08x}")
    }
}

impl<V: CacheValue> StampCache<V> {
    /// The cache saved at `path`, or an empty one.
    pub fn load(path: PathBuf) -> StampCache<V> {
        let known = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut f = l.rsplitn(4, '\t');
                let value = V::parse(f.next()?)?;
                let mtime = f.next()?.parse().ok()?;
                let size = f.next()?.parse().ok()?;
                Some((f.next()?.to_string(), (size, mtime, value)))
            })
            .collect();
        StampCache { path, known, dirty: false }
    }

    /// The value for `file`, if it's known and the file hasn't changed since.
    pub fn get(&self, file: &Path) -> Option<V> {
        let (size, mtime) = stamp(file)?;
        let (s, t, value) = self.known.get(file.to_string_lossy().as_ref())?;
        ((*s, *t) == (size, mtime)).then(|| value.clone())
    }

    pub fn insert(&mut self, file: &Path, value: V) {
        if let Some((size, mtime)) = stamp(file) {
            self.known.insert(file.to_string_lossy().into_owned(), (size, mtime, value));
            self.dirty = true;
        }
    }

    /// Saves it, if anything was added. It's only a cache, so failing to is no matter.
    pub fn save(&self) {
        if !self.dirty {
            return;
        }
        let mut lines: Vec<String> = self
            .known
            .iter()
            .map(|(p, (size, mtime, value))| format!("{p}\t{size}\t{mtime}\t{}", value.write()))
            .collect();
        lines.sort();
        let _ = write_atomic(&self.path, lines.join("\n"));
    }
}

/// A folder of romi's own in /tmp, removed with everything in it when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A new, empty folder whose name has `what` in it. Ones left by romis that died before they
    /// could remove theirs (a core can crash) are removed first.
    pub fn new(what: &str) -> std::io::Result<TempDir> {
        let prefix = format!("romi-{what}-");
        for entry in std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let pid = name.to_str().and_then(|n| n.strip_prefix(&prefix)).and_then(|p| p.parse::<u32>().ok());
            if pid.is_some_and(|pid| !Path::new(&format!("/proc/{pid}")).exists()) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let dir = std::env::temp_dir().join(format!("{prefix}{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(TempDir(dir))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_remember_values_until_the_file_changes() {
        let dir = TempDir::new("cache-test").unwrap();
        let (file, saved) = (dir.path().join("game.gba"), dir.path().join("cache/crc.tsv"));
        std::fs::write(&file, b"rom").unwrap();
        let mut cache = StampCache::<u32>::load(saved.clone());
        cache.insert(&file, 0xab);
        cache.save();
        assert!(std::fs::read_to_string(&saved).unwrap().ends_with("\t000000ab"));
        assert_eq!(StampCache::<u32>::load(saved.clone()).get(&file), Some(0xab));
        std::fs::write(&file, b"a longer rom").unwrap();
        assert_eq!(StampCache::<u32>::load(saved).get(&file), None);
    }
}
