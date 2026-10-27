//! The Kitty graphics protocol, for drawing emulated frames: handing pixels over in shared memory,
//! and Unicode placeholders, which let ratatui lay the picture out like any other text.
//!
//! With placeholders, every cell the picture covers holds U+10EEEE with marks saying which row and
//! column of the picture it shows, coloured with the image id. ratatui draws those cells once, and
//! each new frame only replaces the image behind them.

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::num::NonZeroU16;

const PLACEHOLDER: char = '\u{10EEEE}';

/// The marks that number rows and columns, in order: row 0 is U+0305, row 1 U+030D and so on.
/// From kitty's rowcolumn-diacritics.txt (the same table ratatui-image carries, MIT licensed).
static DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

/// Fills `area` with placeholder cells for image `id`. Only the first cell of each row says where
/// it is; the terminal counts along the row from there.
pub fn placeholders(buf: &mut Buffer, area: Rect, id: u32) {
    let [_, r, g, b] = id.to_be_bytes();
    let fg = Color::Rgb(r, g, b);
    let max = DIACRITICS.len() as u16;
    let mut symbol = String::new();
    for y in 0..area.height.min(max) {
        for x in 0..area.width.min(max) {
            let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) else { continue };
            symbol.clear();
            symbol.push(PLACEHOLDER);
            if x == 0 {
                symbol.push(DIACRITICS[y as usize]);
                symbol.push(DIACRITICS[0]);
            }
            cell.set_symbol(&symbol)
                .set_fg(fg)
                .set_diff_option(CellDiffOption::ForcedWidth(NonZeroU16::new(1).unwrap()));
        }
    }
}

/// Deletes image `id` and everything showing it.
pub fn delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// Writes `data` to a new POSIX shared memory object called `name`, for the terminal to read
/// (and unlink) when it's sent with `t=s`.
pub fn write_shm(name: &str, data: &[u8]) -> std::io::Result<()> {
    let cname = std::ffi::CString::new(name).expect("shm names have no NUL");
    // SAFETY: plain POSIX calls on a fresh object; the mapping is exactly `data.len()` bytes and
    // is unmapped before returning.
    unsafe {
        let fd = libc::shm_open(cname.as_ptr(), libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, 0o600);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let result = (|| {
            if libc::ftruncate(fd, data.len() as libc::off_t) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let ptr = libc::mmap(std::ptr::null_mut(), data.len(), libc::PROT_WRITE, libc::MAP_SHARED, fd, 0);
            if ptr == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.cast(), data.len());
            libc::munmap(ptr, data.len());
            Ok(())
        })();
        libc::close(fd);
        if result.is_err() {
            libc::shm_unlink(cname.as_ptr());
        }
        result
    }
}

/// Unlinks a shared memory object; true if it was still there.
pub fn remove_shm(name: &str) -> bool {
    let cname = std::ffi::CString::new(name).expect("shm names have no NUL");
    // SAFETY: unlinking by name has no memory-safety requirements.
    unsafe { libc::shm_unlink(cname.as_ptr()) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_number_the_first_cell_of_each_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        placeholders(&mut buf, Rect::new(0, 0, 3, 2), 0x726f6d);
        assert_eq!(buf[(0, 0)].symbol(), "\u{10EEEE}\u{305}\u{305}");
        assert_eq!(buf[(0, 1)].symbol(), "\u{10EEEE}\u{30D}\u{305}");
        assert_eq!(buf[(2, 1)].symbol(), "\u{10EEEE}");
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(0x72, 0x6f, 0x6d));
    }
}
