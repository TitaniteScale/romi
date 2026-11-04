//! Save states, in RetroArch's file format, so one made in romi loads in RetroArch and back.
//!
//! A state is what the core hands over (`retro_serialize`), in blocks after an 8-byte header: `MEM `
//! for the core's data, `ACHV` for how far along the achievements are, and `END `. Each block is its
//! 4-byte name and its length, then the data, padded to a multiple of 8. Older RetroArch wrote the
//! core's data alone, and newer RetroArch compresses the file (RZIP). All of those are read; romi
//! writes blocks, uncompressed.

use anyhow::{Context, Result, bail};
use std::io::Read;

const HEADER: &[u8; 8] = b"RASTATE\x01";
const CORE_BLOCK: &[u8; 4] = b"MEM ";
const ACHIEVEMENTS_BLOCK: &[u8; 4] = b"ACHV";
const END_BLOCK: &[u8; 4] = b"END ";

/// What a state file holds.
#[derive(Debug, Default, PartialEq)]
pub struct State {
    /// What the core gave, to give back.
    pub core: Vec<u8>,
    /// How far along the achievements were, if they were being checked.
    pub achievements: Option<Vec<u8>>,
}

fn block(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(name);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(8), 0);
}

/// The contents of a state file. A core's data has to fit RetroArch's 32-bit block length.
pub fn encode(state: &State) -> Result<Vec<u8>> {
    let extra = state.achievements.as_ref().map_or(0, Vec::len);
    if u32::try_from(state.core.len()).is_err() || u32::try_from(extra).is_err() {
        bail!("the state is too big for a state file");
    }
    let mut out = Vec::with_capacity(state.core.len() + extra + 64);
    out.extend_from_slice(HEADER);
    block(&mut out, CORE_BLOCK, &state.core);
    if let Some(achievements) = &state.achievements {
        block(&mut out, ACHIEVEMENTS_BLOCK, achievements);
    }
    block(&mut out, END_BLOCK, &[]);
    Ok(out)
}

/// What's in the contents of a state file, in any of the forms RetroArch writes.
pub fn decode(file: Vec<u8>) -> Result<State> {
    let file = if file.starts_with(RZIP_MAGIC) { unpack(&file)? } else { file };
    if !file.starts_with(b"RASTATE") {
        // RetroArch's format from before blocks: nothing but the core's data.
        return Ok(State { core: file, achievements: None });
    }
    if file.get(7) != Some(&1) {
        bail!("it's from a newer version of RetroArch");
    }
    let mut state = State::default();
    let mut found_core = false;
    let mut rest = &file[8..];
    while rest.len() >= 8 {
        let (name, data) = rest.split_at(8);
        let len = u32::from_le_bytes([name[4], name[5], name[6], name[7]]) as usize;
        if &name[..4] == END_BLOCK {
            break;
        }
        if len > data.len() {
            bail!("it's cut short");
        }
        match &name[..4] {
            n if n == CORE_BLOCK => {
                state.core = data[..len].to_vec();
                found_core = true;
            }
            n if n == ACHIEVEMENTS_BLOCK => state.achievements = Some(data[..len].to_vec()),
            // Others, like RetroArch's replays, aren't romi's business.
            _ => {}
        }
        // The last block's padding may be missing.
        rest = &data[len.next_multiple_of(8).min(data.len())..];
    }
    if !found_core {
        bail!("it has nothing from the core in it");
    }
    Ok(state)
}

/// How RZIP files start: `#RZIPv`, its version (1 is zlib, 2 zstd) and `#`, then the size of each
/// chunk unpacked (32 bits) and of the whole file (64 bits), little-endian. Each chunk is its packed
/// length (32 bits) and the zlib stream.
const RZIP_MAGIC: &[u8] = b"#RZIPv";
const RZIP_HEADER: usize = 20;

fn unpack(file: &[u8]) -> Result<Vec<u8>> {
    if file.len() < RZIP_HEADER || file[7] != b'#' {
        bail!("it's cut short");
    }
    if file[6] != 1 {
        bail!("it's compressed in a way romi can't read yet (RZIP version {})", file[6]);
    }
    let total = u64::from_le_bytes(file[12..20].try_into().context("reading its size")?);
    let mut out = Vec::new();
    let mut rest = &file[RZIP_HEADER..];
    while (out.len() as u64) < total {
        let Some((len, after)) = rest.split_first_chunk::<4>() else { bail!("it's cut short") };
        let len = u32::from_le_bytes(*len) as usize;
        let Some((chunk, after)) = after.split_at_checked(len) else { bail!("it's cut short") };
        flate2::read::ZlibDecoder::new(chunk).read_to_end(&mut out).context("it's damaged")?;
        rest = after;
    }
    if out.len() as u64 != total {
        bail!("it's damaged");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn rzip(data: &[u8], chunk: usize) -> Vec<u8> {
        let mut out = b"#RZIPv\x01#".to_vec();
        out.extend_from_slice(&(chunk as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        for piece in data.chunks(chunk) {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(piece).unwrap();
            let z = z.finish().unwrap();
            out.extend_from_slice(&(z.len() as u32).to_le_bytes());
            out.extend_from_slice(&z);
        }
        out
    }

    #[test]
    fn states_are_laid_out_as_retroarch_lays_them_out() {
        let state = State { core: vec![1, 2, 3], achievements: Some(vec![9; 8]) };
        let mut wanted = b"RASTATE\x01MEM \x03\0\0\0\x01\x02\x03\0\0\0\0\0ACHV\x08\0\0\0".to_vec();
        wanted.extend_from_slice(&[9; 8]);
        wanted.extend_from_slice(b"END \0\0\0\0");
        assert_eq!(encode(&state).unwrap(), wanted);
    }

    #[test]
    fn states_come_back_as_they_went() {
        for state in [
            State { core: vec![7; 13], achievements: None },
            State { core: vec![7; 16], achievements: Some(vec![1, 2, 3]) },
        ] {
            assert_eq!(decode(encode(&state).unwrap()).unwrap(), state);
        }
    }

    #[test]
    fn compressed_and_old_states_are_read_too() {
        let state = State { core: (0..200u8).collect(), achievements: Some(vec![5]) };
        let file = encode(&state).unwrap();
        assert_eq!(decode(rzip(&file, 64)).unwrap(), state, "in several chunks");
        assert_eq!(decode(vec![1, 2, 3]).unwrap(), State { core: vec![1, 2, 3], achievements: None });
    }

    #[test]
    fn blocks_romi_doesnt_know_are_passed_over_and_the_last_padding_can_be_missing() {
        let mut file = b"RASTATE\x01RPLY\x02\0\0\0\xAA\xBB\0\0\0\0\0\0MEM \x03\0\0\0\x01\x02\x03".to_vec();
        assert_eq!(decode(file.clone()).unwrap().core, vec![1, 2, 3]);
        file.extend_from_slice(&[0; 5]);
        file.extend_from_slice(b"END \0\0\0\0ACHV");
        assert_eq!(decode(file).unwrap().achievements, None, "nothing after the end counts");
    }

    #[test]
    fn broken_files_say_so_and_never_reach_the_core() {
        let error = |file: Vec<u8>| decode(file).unwrap_err().to_string();
        assert_eq!(error(b"RASTATE\x02MEM \0\0\0\0".to_vec()), "it's from a newer version of RetroArch");
        assert_eq!(error(b"RASTATE\x01END \0\0\0\0".to_vec()), "it has nothing from the core in it");
        assert_eq!(error(b"RASTATE\x01MEM \xFF\0\0\0\x01".to_vec()), "it's cut short");
        let whole = rzip(&encode(&State { core: vec![1; 64], achievements: None }).unwrap(), 32);
        assert_eq!(error(whole[..whole.len() - 3].to_vec()), "it's cut short");
        assert!(error(b"#RZIPv\x02#\0\0\0\0\0\0\0\0\0\0\0\0".to_vec()).contains("can't read yet"));
    }
}
