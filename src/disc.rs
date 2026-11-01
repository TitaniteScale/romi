//! Reading what a PlayStation 2 disc says about itself, from an `.iso` of 2048-byte sectors (what
//! PS2 dumps are, bar the odd `.bin`): its serial, from the boot line of `SYSTEM.CNF`.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const SECTOR: u64 = 2048;

/// The serial a PS2 disc image boots, like `SCUS_972.11`, or None if it isn't one of the images
/// this reads (`.iso`), or has no `SYSTEM.CNF` with a `BOOT2` line.
pub fn ps2_serial(path: &Path) -> Option<String> {
    if !path.extension()?.eq_ignore_ascii_case("iso") {
        return None;
    }
    serial_from(std::fs::File::open(path).ok()?)
}

fn serial_from(mut disc: impl Read + Seek) -> Option<String> {
    // The primary volume descriptor is sector 16; its root directory record starts at byte 156.
    let pvd = read_at(&mut disc, 16 * SECTOR, SECTOR as usize)?;
    if pvd.get(1..6)? != b"CD001" {
        return None;
    }
    let (root, root_len) = (u32le(&pvd, 156 + 2)?, u32le(&pvd, 156 + 10)?);
    // Directories are small; one that isn't isn't worth reading for this.
    let dir = read_at(&mut disc, u64::from(root) * SECTOR, root_len.min(1 << 20) as usize)?;
    let mut at = 0;
    while at < dir.len() {
        let len = usize::from(dir[at]);
        if len == 0 {
            // Records don't cross sectors; the rest of this one is padding.
            at = (at / SECTOR as usize + 1) * SECTOR as usize;
            continue;
        }
        let name_len = usize::from(*dir.get(at + 32)?);
        let name = dir.get(at + 33..at + 33 + name_len)?;
        if name.starts_with(b"SYSTEM.CNF") {
            let (lba, size) = (u32le(&dir, at + 2)?, u32le(&dir, at + 10)?);
            let cnf = read_at(&mut disc, u64::from(lba) * SECTOR, size.min(4096) as usize)?;
            return boot_serial(&String::from_utf8_lossy(&cnf));
        }
        at += len;
    }
    None
}

/// `SCUS_972.11` from `BOOT2 = cdrom0:\SCUS_972.11;1`.
fn boot_serial(cnf: &str) -> Option<String> {
    let line = cnf.lines().find(|l| l.trim_start().starts_with("BOOT2"))?;
    let path = line.split_once('=')?.1.trim();
    let file = path.rsplit(['\\', '/', ':']).next()?;
    let serial = file.split(';').next()?.trim();
    (!serial.is_empty()).then(|| serial.to_string())
}

fn u32le(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_at(disc: &mut (impl Read + Seek), at: u64, len: usize) -> Option<Vec<u8>> {
    disc.seek(SeekFrom::Start(at)).ok()?;
    let mut bytes = vec![0; len];
    disc.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A disc image with just enough on it: the volume descriptor, a root directory with a file
    /// or two, and `SYSTEM.CNF` with the given text.
    pub fn image(cnf: &str) -> Vec<u8> {
        let mut disc = vec![0u8; 20 * SECTOR as usize];
        let sector = |n: usize| n * SECTOR as usize;
        disc[sector(16) + 1..sector(16) + 6].copy_from_slice(b"CD001");
        // Root directory at sector 18, a sector long.
        disc[sector(16) + 158..sector(16) + 162].copy_from_slice(&18u32.to_le_bytes());
        disc[sector(16) + 166..sector(16) + 170].copy_from_slice(&(SECTOR as u32).to_le_bytes());
        let mut at = sector(18);
        for (name, lba, size) in [("BOOT.ELF;1", 19u32, 4u32), ("SYSTEM.CNF;1", 19, cnf.len() as u32)] {
            let len = 33 + name.len() + (name.len() + 1) % 2;
            disc[at] = len as u8;
            disc[at + 2..at + 6].copy_from_slice(&lba.to_le_bytes());
            disc[at + 10..at + 14].copy_from_slice(&size.to_le_bytes());
            disc[at + 32] = name.len() as u8;
            disc[at + 33..at + 33 + name.len()].copy_from_slice(name.as_bytes());
            at += len;
        }
        disc[sector(19)..sector(19) + cnf.len()].copy_from_slice(cnf.as_bytes());
        disc
    }

    #[test]
    fn the_serial_is_the_boot_files_name() {
        let cnf = "BOOT2 = cdrom0:\\SCUS_972.11;1\r\nVER = 2.00\r\nVMODE = NTSC\r\n";
        assert_eq!(serial_from(std::io::Cursor::new(image(cnf))), Some("SCUS_972.11".into()));
    }

    #[test]
    fn a_disc_without_one_has_none() {
        assert_eq!(serial_from(std::io::Cursor::new(image("BOOT = cdrom:\\SLUS_000.01;1\r\n"))), None);
        assert_eq!(serial_from(std::io::Cursor::new(vec![0u8; 40 * SECTOR as usize])), None);
        assert_eq!(serial_from(std::io::Cursor::new(Vec::new())), None);
    }
}
