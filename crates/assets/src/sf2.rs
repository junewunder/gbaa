//! A SoundFont 2 writer: presets of one instrument each, its zones key
//! ranges over 16-bit mono samples. Just what [`crate::samples`] needs to
//! play a game's MIDI with its own instruments.

/// SF2 generator operators used here.
pub mod op {
    pub const PAN: u16 = 17;
    pub const ATTACK_VOL_ENV: u16 = 34;
    pub const DECAY_VOL_ENV: u16 = 36;
    pub const SUSTAIN_VOL_ENV: u16 = 37;
    pub const RELEASE_VOL_ENV: u16 = 38;
    pub const INSTRUMENT: u16 = 41;
    pub const KEY_RANGE: u16 = 43;
    pub const COARSE_TUNE: u16 = 51;
    pub const SAMPLE_ID: u16 = 53;
    pub const SAMPLE_MODES: u16 = 54;
    pub const SCALE_TUNING: u16 = 56;
    pub const OVERRIDING_ROOT_KEY: u16 = 58;
}

pub struct Sample {
    pub name: String,
    pub data: Vec<i16>,
    pub rate: u32,
    pub root: u8,
    /// Cents.
    pub correction: i8,
    /// Start and end (exclusive) of the loop.
    pub looped: Option<(u32, u32)>,
}

pub struct Zone {
    pub keys: (u8, u8),
    pub sample: usize,
    /// Other generators, (operator, amount).
    pub gens: Vec<(u16, i16)>,
}

pub struct Preset {
    pub name: String,
    pub bank: u16,
    pub program: u16,
    pub zones: Vec<Zone>,
}

fn name20(s: &str) -> [u8; 20] {
    let mut b = [0u8; 20];
    for (d, c) in b.iter_mut().zip(s.bytes().take(19)) {
        *d = c;
    }
    b
}

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend((body.len() as u32).to_le_bytes());
    v.extend(body);
    if body.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn list(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut body = kind.to_vec();
    for c in chunks {
        body.extend(c);
    }
    chunk(b"LIST", &body)
}

fn zstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    if v.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// The whole file.
pub fn write(name: &str, samples: &[Sample], presets: &[Preset]) -> Vec<u8> {
    let mut ifil = Vec::new();
    ifil.extend(2u16.to_le_bytes());
    ifil.extend(1u16.to_le_bytes());
    let info = list(
        b"INFO",
        &[
            chunk(b"ifil", &ifil),
            chunk(b"isng", &zstr("EMU8000")),
            chunk(b"INAM", &zstr(name)),
            chunk(b"ISFT", &zstr("gbaa-assets")),
        ],
    );

    // Sample data, each followed by 46 zero points as the spec asks.
    let (mut smpl, mut shdr) = (Vec::new(), Vec::new());
    let mut at = 0u32;
    for s in samples {
        let start = at;
        for v in &s.data {
            smpl.extend(v.to_le_bytes());
        }
        smpl.extend([0u8; 92]);
        let end = start + s.data.len() as u32;
        at = end + 46;
        let (ls, le) = s
            .looped
            .map_or((start, end), |(a, b)| (start + a, start + b));
        shdr.extend(name20(&s.name));
        for w in [start, end, ls, le, s.rate] {
            shdr.extend(w.to_le_bytes());
        }
        shdr.push(s.root);
        shdr.push(s.correction as u8);
        shdr.extend(0u16.to_le_bytes());
        shdr.extend(1u16.to_le_bytes());
    }
    shdr.extend(name20("EOS"));
    shdr.extend([0u8; 26]);
    let sdta = list(b"sdta", &[chunk(b"smpl", &smpl)]);

    let gen_rec = |v: &mut Vec<u8>, op: u16, amount: [u8; 2]| {
        v.extend(op.to_le_bytes());
        v.extend(amount);
    };
    let (mut phdr, mut pbag, mut pgen) = (Vec::new(), Vec::new(), Vec::new());
    let (mut inst, mut ibag, mut igen) = (Vec::new(), Vec::new(), Vec::new());
    let (mut pgen_n, mut igen_n, mut ibag_n) = (0u16, 0u16, 0u16);
    for (i, p) in presets.iter().enumerate() {
        phdr.extend(name20(&p.name));
        phdr.extend(p.program.to_le_bytes());
        phdr.extend(p.bank.to_le_bytes());
        phdr.extend((i as u16).to_le_bytes());
        phdr.extend([0u8; 12]);
        pbag.extend(pgen_n.to_le_bytes());
        pbag.extend(0u16.to_le_bytes());
        gen_rec(&mut pgen, op::INSTRUMENT, (i as u16).to_le_bytes());
        pgen_n += 1;

        inst.extend(name20(&p.name));
        inst.extend(ibag_n.to_le_bytes());
        for z in &p.zones {
            ibag.extend(igen_n.to_le_bytes());
            ibag.extend(0u16.to_le_bytes());
            ibag_n += 1;
            gen_rec(&mut igen, op::KEY_RANGE, [z.keys.0, z.keys.1]);
            for &(op, amount) in &z.gens {
                gen_rec(&mut igen, op, amount.to_le_bytes());
            }
            gen_rec(&mut igen, op::SAMPLE_ID, (z.sample as u16).to_le_bytes());
            igen_n += 2 + z.gens.len() as u16;
        }
    }
    phdr.extend(name20("EOP"));
    phdr.extend([0u8; 4]);
    phdr.extend((presets.len() as u16).to_le_bytes());
    phdr.extend([0u8; 12]);
    pbag.extend(pgen_n.to_le_bytes());
    pbag.extend(0u16.to_le_bytes());
    pgen.extend([0u8; 4]);
    inst.extend(name20("EOI"));
    inst.extend(ibag_n.to_le_bytes());
    ibag.extend(igen_n.to_le_bytes());
    ibag.extend(0u16.to_le_bytes());
    igen.extend([0u8; 4]);
    let pdta = list(
        b"pdta",
        &[
            chunk(b"phdr", &phdr),
            chunk(b"pbag", &pbag),
            chunk(b"pmod", &[0u8; 10]),
            chunk(b"pgen", &pgen),
            chunk(b"inst", &inst),
            chunk(b"ibag", &ibag),
            chunk(b"imod", &[0u8; 10]),
            chunk(b"igen", &igen),
            chunk(b"shdr", &shdr),
        ],
    );
    let mut body = b"sfbk".to_vec();
    body.extend(info);
    body.extend(sdta);
    body.extend(pdta);
    chunk(b"RIFF", &body)
}
