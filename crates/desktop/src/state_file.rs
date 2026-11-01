//! Reading mGBA state files. `runtime::state` loads the uncompressed
//! `GBASerializedState`, which `mgba-dump --save-state` writes. mGBA's GUI
//! writes its `.ss` files as PNGs instead (`src/core/serialize.c`, 0.10):
//! the screenshot as the image, the state zlib-compressed in a `gbAs`
//! chunk, and each extra item (save data, RTC, metadata) in a `gbAx` chunk
//! as its tag and uncompressed size (little-endian words), then its data
//! zlib-compressed. This unpacks the state and the save data; the rest is
//! skipped. Loading such a state, mGBA also puts its save data in place of
//! the cartridge's, so both runs here do too.

use std::path::Path;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
/// mGBA's `EXTDATA_SAVEDATA`.
const EXTDATA_SAVEDATA: u32 = 2;
/// Bounds what a chunk may inflate to: a state is 0x61000 bytes and save
/// data at most 128 KB.
const MAX_ITEM: usize = 1 << 20;

/// A state file's contents.
#[derive(Debug)]
pub struct StateFile {
    /// The `GBASerializedState`, perhaps with extra data after it.
    pub state: Vec<u8>,
    /// The cartridge save data stored with it (only a PNG state has it).
    pub save: Option<Vec<u8>>,
}

/// Read the state at `path`, raw or PNG.
pub fn read(path: &Path) -> Result<StateFile, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    unpack(bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// A raw state as it is; a PNG state's `gbAs` and save data, inflated.
pub fn unpack(bytes: Vec<u8>) -> Result<StateFile, String> {
    let Some(mut rest) = bytes.strip_prefix(PNG_MAGIC) else {
        return Ok(StateFile {
            state: bytes,
            save: None,
        });
    };
    let (mut state, mut save) = (None, None);
    while !rest.is_empty() {
        let bad = || "a PNG cut short or malformed".to_string();
        let len = u32::from_be_bytes(rest.get(..4).ok_or_else(bad)?.try_into().unwrap()) as usize;
        let kind = rest.get(4..8).ok_or_else(bad)?;
        let data = rest.get(8..8 + len).ok_or_else(bad)?;
        match kind {
            b"gbAs" => state = Some(inflate(data, "state")?),
            b"gbAx" if data.len() >= 8 => {
                let tag = u32::from_le_bytes(data[..4].try_into().unwrap());
                let size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
                if tag == EXTDATA_SAVEDATA {
                    let s = inflate(&data[8..], "save data")?;
                    if s.len() != size {
                        return Err(format!("save data is {} bytes, not {size}", s.len()));
                    }
                    save = Some(s);
                }
            }
            b"IEND" => break,
            _ => {}
        }
        // The chunk's CRC follows its data.
        rest = rest.get(12 + len..).ok_or_else(bad)?;
    }
    let state = state.ok_or("a PNG with no mGBA state in it (no gbAs chunk)")?;
    Ok(StateFile { state, save })
}

fn inflate(data: &[u8], what: &str) -> Result<Vec<u8>, String> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, MAX_ITEM)
        .map_err(|e| format!("the PNG's {what} doesn't inflate ({:?})", e.status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::deflate::compress_to_vec_zlib;

    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0; 4]);
        c
    }

    #[test]
    fn raw_state_passes_through() {
        let f = unpack(vec![1, 2, 3]).unwrap();
        assert_eq!(f.state, [1, 2, 3]);
        assert!(f.save.is_none());
    }

    #[test]
    fn png_state_and_save_data_unpack() {
        let state = vec![7u8; 5000];
        let save = vec![9u8; 0x8000];
        let mut x = 2u32.to_le_bytes().to_vec();
        x.extend_from_slice(&0x8000u32.to_le_bytes());
        x.extend_from_slice(&compress_to_vec_zlib(&save, 6));
        let mut rtc = 4u32.to_le_bytes().to_vec();
        rtc.extend_from_slice(&[0; 12]);
        let mut png = PNG_MAGIC.to_vec();
        png.extend(chunk(b"IHDR", &[0; 13]));
        png.extend(chunk(b"gbAs", &compress_to_vec_zlib(&state, 6)));
        png.extend(chunk(b"gbAx", &rtc));
        png.extend(chunk(b"gbAx", &x));
        png.extend(chunk(b"IEND", &[]));
        let f = unpack(png).unwrap();
        assert_eq!(f.state, state);
        assert_eq!(f.save, Some(save));
    }

    #[test]
    fn png_without_state_is_refused() {
        let mut png = PNG_MAGIC.to_vec();
        png.extend(chunk(b"IEND", &[]));
        assert!(unpack(png).unwrap_err().contains("no gbAs"));
        let mut cut = PNG_MAGIC.to_vec();
        cut.extend(&chunk(b"gbAs", &[1, 2, 3])[..9]);
        assert!(unpack(cut).unwrap_err().contains("cut short"));
    }
}
