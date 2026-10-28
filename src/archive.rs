//! Reading ROMs out of zips, which is all romi needs of archives: no zip64, and only stored or
//! deflated files, which is what ROM sets use.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A file in a zip, from its central directory.
pub struct ZipEntry {
    /// Its name, without the folders it's in.
    pub name: String,
    pub crc: u32,
    pub size: u32,
    method: usize,
    packed: u32,
    offset: u32,
}

fn le16(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The files in a zip.
pub fn entries(path: &Path) -> Option<Vec<ZipEntry>> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let tail_len = len.min(66_000);
    f.seek(SeekFrom::Start(len - tail_len)).ok()?;
    let mut tail = vec![0; tail_len as usize];
    f.read_exact(&mut tail).ok()?;
    let eocd = tail.windows(4).rposition(|w| w == b"PK\x05\x06")?;
    let (cd_size, cd_offset) = (le32(&tail, eocd + 12)?, le32(&tail, eocd + 16)?);
    if cd_offset == u32::MAX || cd_size as u64 > len {
        return None; // zip64
    }
    f.seek(SeekFrom::Start(cd_offset as u64)).ok()?;
    let mut cd = vec![0; cd_size as usize];
    f.read_exact(&mut cd).ok()?;
    let (mut at, mut out) = (0, Vec::new());
    while cd.get(at..at + 4) == Some(b"PK\x01\x02") {
        let name_len = le16(&cd, at + 28)?;
        let name = String::from_utf8_lossy(cd.get(at + 46..at + 46 + name_len)?);
        out.push(ZipEntry {
            name: name.rsplit(['/', '\\']).next().unwrap_or_default().to_string(),
            crc: le32(&cd, at + 16)?,
            size: le32(&cd, at + 24)?,
            method: le16(&cd, at + 10)?,
            packed: le32(&cd, at + 20)?,
            offset: le32(&cd, at + 42)?,
        });
        at += 46 + name_len + le16(&cd, at + 30)? + le16(&cd, at + 32)?;
    }
    Some(out)
}

/// The biggest file in a zip, which is the game in the zips ROM sets come in (the first, if two
/// are the same size).
pub fn biggest(path: &Path) -> Option<ZipEntry> {
    entries(path)?.into_iter().reduce(|best, e| if e.size > best.size { e } else { best })
}

/// A file's contents, decompressed.
pub fn read(path: &Path, entry: &ZipEntry) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let mut local = [0u8; 30];
    f.seek(SeekFrom::Start(entry.offset as u64)).ok()?;
    f.read_exact(&mut local).ok()?;
    let data_at = entry.offset as u64 + 30 + le16(&local, 26)? as u64 + le16(&local, 28)? as u64;
    f.seek(SeekFrom::Start(data_at)).ok()?;
    let packed = f.take(entry.packed as u64);
    let mut out = Vec::with_capacity(entry.size as usize);
    match entry.method {
        0 => packed.take(entry.size as u64).read_to_end(&mut out).ok()?,
        8 => flate2::read::DeflateDecoder::new(packed).read_to_end(&mut out).ok()?,
        _ => return None,
    };
    Some(out)
}

/// The biggest file in a zip and its contents, if it's at most `max` bytes.
pub fn read_biggest(path: &Path, max: u64) -> Option<(String, Vec<u8>)> {
    let entry = biggest(path).filter(|e| u64::from(e.size) <= max)?;
    let data = read(path, &entry)?;
    Some((entry.name, data))
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
        let dir = crate::files::TempDir::new("zip-test").unwrap();
        let path = dir.path().join("game.zip");
        std::fs::write(&path, zip(&[("readme.txt", b"hi"), ("roms/Game (USA).md", b"SEGA GENESIS")])).unwrap();
        assert_eq!(read_biggest(&path, 1 << 20), Some(("Game (USA).md".to_string(), b"SEGA GENESIS".to_vec())));
        assert_eq!(read_biggest(&path, 4), None);
        assert_eq!(biggest(&path).map(|e| e.crc), Some(crc32fast::hash(b"SEGA GENESIS")));
    }
}
