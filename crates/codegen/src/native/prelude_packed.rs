// `Ram` (printed after the prelude) at the play layout (`--layout packed`,
// native/lift-packed-layout): the globals are fields in the lifter's order,
// not at their guest offsets, and `bytes` is the rest of EWRAM and IWRAM.
// `RAM_GLOBALS` places each: its RAM offset, its size, its field's offset in
// `Ram`. A byte at a RAM offset (`g.ram[o]`: computed addresses, the stack,
// the BIOS calls) is the field's that holds it, else `bytes`'; the fields
// are little-endian, as the GBA is.
const _: () = assert!(cfg!(target_endian = "little"));

impl Ram {
    /// Where RAM offset `o` is in `Ram`, in bytes from its start.
    #[inline(always)]
    fn at(o: usize) -> usize {
        assert!(o < EWRAM + IWRAM, "RAM offset {o:#x} out of range");
        let i = RAM_GLOBALS.partition_point(|&(lo, _, _)| lo <= o);
        match i.checked_sub(1).map(|i| RAM_GLOBALS[i]) {
            Some((lo, n, f)) if o < lo + n => f + (o - lo),
            _ => std::mem::offset_of!(Ram, bytes) + o,
        }
    }

    /// Where `n` bytes from RAM offset `o` are in `Ram`, if in one piece:
    /// inside one global, or in `bytes` between them.
    #[inline(always)]
    fn span(o: usize, n: usize) -> Option<usize> {
        let i = RAM_GLOBALS.partition_point(|&(lo, _, _)| lo < o + n);
        match i.checked_sub(1).map(|i| RAM_GLOBALS[i]) {
            Some((lo, len, f)) if lo + len > o => (lo <= o && o + n <= lo + len).then(|| f + (o - lo)),
            _ => Some(std::mem::offset_of!(Ram, bytes) + o),
        }
    }

    /// EWRAM then IWRAM as the guest lays them out: what the native check
    /// records.
    fn image(&self) -> Vec<u8> {
        let mut v = self.bytes.to_vec();
        for &(lo, n, _) in &RAM_GLOBALS {
            for o in lo..lo + n {
                v[o] = self[o];
            }
        }
        v
    }
}

impl std::ops::Index<usize> for Ram {
    type Output = u8;
    fn index(&self, o: usize) -> &u8 {
        // SAFETY: `at` is inside a field of integers, in `Ram`.
        unsafe { &*(self as *const Ram).cast::<u8>().add(Ram::at(o)) }
    }
}

impl std::ops::IndexMut<usize> for Ram {
    fn index_mut(&mut self, o: usize) -> &mut u8 {
        // SAFETY: as `index`; any byte written leaves a valid integer.
        unsafe { &mut *(self as *mut Ram).cast::<u8>().add(Ram::at(o)) }
    }
}

fn record(g: &mut Game) {
    native_host::record(&g.hw, &g.ram.image());
}

