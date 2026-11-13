//! The native check's record (docs/native-port-design.md, section 7):
//! each frame's picture hash, the RAM, the I/O registers, the display's
//! memories and the frame's audio, as both builds hold them when the
//! frame is presented (the start of V-blank, before its handlers run).
//!
//! A file is [`MAGIC`], then one record a frame: the frame number and
//! picture hash (`u64` each, little-endian), the audio (`u8` 1, the
//! sample count `u32`, the FNV-1a 64 hash `u64` of the frame's samples and
//! `u8` 1 if every sample was zero; or `u8` 0 for a build with no audio), then each [`Region`] in order: `u8`
//! 1 and its bytes when they changed since the frame before, else `u8` 0.
//! So a run whose VRAM is set once costs its VRAM once.
//!
//! `io` is the I/O block as halfwords, little-endian, at their offsets:
//! each register's stored value (write-only bits included; the play
//! build's `Io::raw16`), but `DISPSTAT` its writable bits and `VCOUNT`
//! the beam's line. Which offsets a check compares is the check's
//! business (`route-check --native`).

pub mod stop;

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"gbaarec1";

/// The regions a record holds, in file order, with their sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Ewram,
    Iwram,
    Io,
    Palette,
    Vram,
    Oam,
}

impl Region {
    pub const ALL: [Region; 6] = [
        Region::Ewram,
        Region::Iwram,
        Region::Io,
        Region::Palette,
        Region::Vram,
        Region::Oam,
    ];

    pub const fn size(self) -> usize {
        match self {
            Region::Ewram => 0x4_0000,
            Region::Iwram => 0x8000,
            Region::Io | Region::Palette | Region::Oam => 0x400,
            Region::Vram => 0x1_8000,
        }
    }

    /// The GBA address of the region's first byte.
    pub const fn base(self) -> u32 {
        match self {
            Region::Ewram => 0x0200_0000,
            Region::Iwram => 0x0300_0000,
            Region::Io => 0x0400_0000,
            Region::Palette => 0x0500_0000,
            Region::Vram => 0x0600_0000,
            Region::Oam => 0x0700_0000,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Region::Ewram => "EWRAM",
            Region::Iwram => "IWRAM",
            Region::Io => "I/O",
            Region::Palette => "palette",
            Region::Vram => "VRAM",
            Region::Oam => "OAM",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One frame's audio: how many samples it made, their hash, and whether
/// they were all zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audio {
    pub samples: u32,
    pub hash: u64,
    pub silent: bool,
}

/// One frame's record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// From 1, as `--frames` counts.
    pub frame: u64,
    /// `desktop::frame_hash` of the picture.
    pub hash: u64,
    /// `None`: the build makes no audio.
    pub audio: Option<Audio>,
    /// Each [`Region`]'s bytes, in [`Region::ALL`]'s order.
    pub regions: [Box<[u8]>; 6],
}

impl Frame {
    /// Frame 0, every region zero.
    pub fn new() -> Frame {
        Frame {
            frame: 0,
            hash: 0,
            audio: None,
            regions: Region::ALL.map(|r| vec![0; r.size()].into_boxed_slice()),
        }
    }

    pub fn region(&self, r: Region) -> &[u8] {
        &self.regions[r.index()]
    }

    pub fn region_mut(&mut self, r: Region) -> &mut [u8] {
        &mut self.regions[r.index()]
    }

    /// The I/O halfword at offset `o`.
    pub fn io16(&self, o: u32) -> u16 {
        let b = &self.region(Region::Io)[o as usize..];
        u16::from_le_bytes([b[0], b[1]])
    }

    pub fn set_io16(&mut self, o: u32, v: u16) {
        self.region_mut(Region::Io)[o as usize..o as usize + 2].copy_from_slice(&v.to_le_bytes());
    }
}

impl Default for Frame {
    fn default() -> Frame {
        Frame::new()
    }
}

/// FNV-1a 64 over `bytes`, folded into `h` (start from [`FNV`]).
pub fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// FNV-1a 64's start.
pub const FNV: u64 = 0xcbf2_9ce4_8422_2325;

/// Writes records, each region only when it changed.
pub struct Writer<W: Write> {
    out: W,
    last: Frame,
}

impl Writer<BufWriter<File>> {
    pub fn create(path: &Path) -> io::Result<Self> {
        Writer::new(BufWriter::new(File::create(path)?))
    }
}

impl<W: Write> Writer<W> {
    pub fn new(mut out: W) -> io::Result<Self> {
        out.write_all(MAGIC)?;
        Ok(Writer {
            out,
            last: Frame::new(),
        })
    }

    pub fn write(&mut self, f: &Frame) -> io::Result<()> {
        self.out.write_all(&f.frame.to_le_bytes())?;
        self.out.write_all(&f.hash.to_le_bytes())?;
        match f.audio {
            Some(a) => {
                self.out.write_all(&[1])?;
                self.out.write_all(&a.samples.to_le_bytes())?;
                self.out.write_all(&a.hash.to_le_bytes())?;
                self.out.write_all(&[a.silent as u8])?;
            }
            None => self.out.write_all(&[0])?,
        }
        for r in Region::ALL {
            let (new, old) = (f.region(r), self.last.region_mut(r));
            if new == &old[..] {
                self.out.write_all(&[0])?;
            } else {
                self.out.write_all(&[1])?;
                self.out.write_all(new)?;
                old.copy_from_slice(new);
            }
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Reads records back, one frame at a time.
pub struct Reader<R: Read> {
    input: R,
    frame: Frame,
}

impl Reader<BufReader<File>> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Reader::new(BufReader::new(File::open(path)?))
    }
}

impl<R: Read> Reader<R> {
    pub fn new(mut input: R) -> io::Result<Self> {
        let mut m = [0; 8];
        input.read_exact(&mut m)?;
        if &m != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a native-check record",
            ));
        }
        Ok(Reader {
            input,
            frame: Frame::new(),
        })
    }

    /// The next frame's record, or `None` at the end.
    pub fn read_frame(&mut self) -> io::Result<Option<&Frame>> {
        let mut n = [0; 8];
        match self.input.read_exact(&mut n) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let f = &mut self.frame;
        f.frame = u64::from_le_bytes(n);
        self.input.read_exact(&mut n)?;
        f.hash = u64::from_le_bytes(n);
        let mut b = [0; 1];
        self.input.read_exact(&mut b)?;
        f.audio = if b[0] == 1 {
            let (mut s, mut h, mut z) = ([0; 4], [0; 8], [0; 1]);
            self.input.read_exact(&mut s)?;
            self.input.read_exact(&mut h)?;
            self.input.read_exact(&mut z)?;
            Some(Audio {
                samples: u32::from_le_bytes(s),
                hash: u64::from_le_bytes(h),
                silent: z[0] == 1,
            })
        } else {
            None
        };
        for r in Region::ALL {
            self.input.read_exact(&mut b)?;
            if b[0] == 1 {
                self.input.read_exact(&mut f.regions[r.index()])?;
            }
        }
        Ok(Some(&self.frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_what_it_wrote() {
        let mut w = Writer::new(Vec::new()).unwrap();
        let mut a = Frame::new();
        a.frame = 1;
        a.hash = 7;
        a.region_mut(Region::Iwram)[5] = 9;
        a.set_io16(0x82, 0x1234);
        w.write(&a).unwrap();
        let mut b = a.clone();
        b.frame = 2;
        b.audio = Some(Audio {
            samples: 3,
            hash: 4,
            silent: false,
        });
        w.write(&b).unwrap();
        // The first record stores the two regions that aren't zero, the
        // second none: none changed.
        let one = 8 + 8 + 1 + 6 + Region::Iwram.size() + Region::Io.size();
        let two = 8 + 8 + 1 + 13 + 6;
        assert_eq!(w.out.len(), 8 + one + two);
        let mut r = Reader::new(&w.out[..]).unwrap();
        assert_eq!(r.read_frame().unwrap(), Some(&a));
        assert_eq!(r.read_frame().unwrap(), Some(&b));
        assert_eq!(r.read_frame().unwrap(), None);
        assert_eq!(b.io16(0x82), 0x1234);
    }
}
