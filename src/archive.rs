//! Archives: reading the ROM out of a zip, and folders in /tmp that clean up after themselves.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The biggest file in a zip, decompressed, and its name (without folders), if it's at most `max`
/// bytes. Stored and deflated files only, which is what ROM zips use.
pub fn biggest_in_zip(path: &Path, max: u64) -> Option<(String, Vec<u8>)> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let tail_len = len.min(66_000);
    f.seek(SeekFrom::Start(len - tail_len)).ok()?;
    let mut tail = vec![0; tail_len as usize];
    f.read_exact(&mut tail).ok()?;
    let eocd = tail.windows(4).rposition(|w| w == b"PK\x05\x06")?;
    let le32 = |b: &[u8], at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    let le16 = |b: &[u8], at: usize| Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize);
    let (cd_size, cd_offset) = (le32(&tail, eocd + 12)?, le32(&tail, eocd + 16)?);
    if cd_offset == u32::MAX || cd_size as u64 > len {
        return None; // zip64; not worth handling for ROMs
    }
    f.seek(SeekFrom::Start(cd_offset as u64)).ok()?;
    let mut cd = vec![0; cd_size as usize];
    f.read_exact(&mut cd).ok()?;
    // (method, compressed size, size, local header offset, name) of the biggest entry.
    let (mut at, mut best) = (0, None::<(usize, u32, u32, u32, &[u8])>);
    while cd.get(at..at + 4) == Some(b"PK\x01\x02") {
        let name_len = le16(&cd, at + 28)?;
        let name = cd.get(at + 46..at + 46 + name_len)?;
        let entry = (le16(&cd, at + 10)?, le32(&cd, at + 20)?, le32(&cd, at + 24)?, le32(&cd, at + 42)?, name);
        if best.is_none_or(|b| entry.2 > b.2) {
            best = Some(entry);
        }
        at += 46 + name_len + le16(&cd, at + 30)? + le16(&cd, at + 32)?;
    }
    let (method, packed, size, offset, name) = best?;
    if u64::from(size) > max {
        return None;
    }
    let name = String::from_utf8_lossy(name);
    let name = name.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
    let mut local = [0u8; 30];
    f.seek(SeekFrom::Start(offset as u64)).ok()?;
    f.read_exact(&mut local).ok()?;
    let data_at = offset as u64 + 30 + le16(&local, 26)? as u64 + le16(&local, 28)? as u64;
    f.seek(SeekFrom::Start(data_at)).ok()?;
    let packed = f.take(packed as u64);
    let mut out = Vec::with_capacity(size as usize);
    match method {
        0 => packed.take(size as u64).read_to_end(&mut out).ok()?,
        8 => flate2::read::DeflateDecoder::new(packed).read_to_end(&mut out).ok()?,
        _ => return None,
    };
    Some((name, out))
}

/// A folder of romi's own in /tmp, removed with everything in it when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A new, empty folder whose name has `what` in it.
    pub fn new(what: &str) -> std::io::Result<TempDir> {
        let dir = std::env::temp_dir().join(format!("romi-{what}-{}", std::process::id()));
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

    /// A zip of stored (uncompressed) files.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let crc = crc32fast::hash(data);
            let offset = out.len() as u32;
            let sizes = [crc, data.len() as u32, data.len() as u32];
            out.extend_from_slice(b"PK\x03\x04\x14\0\0\0\0\0\0\0\0\0");
            sizes.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            dir.extend_from_slice(b"PK\x01\x02\x14\0\x14\0\0\0\0\0\0\0\0\0");
            sizes.iter().for_each(|v| dir.extend_from_slice(&v.to_le_bytes()));
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&offset.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let (dir_at, dir_len, count) = (out.len() as u32, dir.len() as u32, files.len() as u16);
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&[count.to_le_bytes(), count.to_le_bytes()].concat());
        out.extend_from_slice(&[dir_len.to_le_bytes(), dir_at.to_le_bytes()].concat());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn the_biggest_file_comes_out_with_its_name() {
        let dir = TempDir::new("zip-test").unwrap();
        let path = dir.path().join("game.zip");
        std::fs::write(&path, zip(&[("readme.txt", b"hi"), ("roms/Game (USA).md", b"SEGA GENESIS")])).unwrap();
        assert_eq!(biggest_in_zip(&path, 1 << 20), Some(("Game (USA).md".to_string(), b"SEGA GENESIS".to_vec())));
        assert_eq!(biggest_in_zip(&path, 4), None);
    }
}
