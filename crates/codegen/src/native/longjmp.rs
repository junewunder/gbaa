//! Context saves and restores, `setjmp` and `longjmp` (native/lift-longjmp).
//!
//! newlib's `setjmp` stores r4–r12, sp and lr through r0 (`stm r0!,
//! {r4-r12, sp, lr}`) and returns 0; `longjmp` loads them back (`ldm r0!`,
//! the same registers), so its return goes into the frame of the function
//! that called `setjmp`, at that call's return site, with r0 the value
//! passed. Natively that frame's r4–r11 and sp are Rust locals, out of the
//! restore's reach, so the lift does it the Rust way:
//!
//! - The restore's function returns by unwinding: its `ldm` keeps the
//!   buffer's address (`ctx_buf`), and each of its returns after it is the
//!   prelude's `restore(ctx_buf, [r0, r1, r2, r3])`, a panic carrying them.
//! - A function that calls a save directly (`setjmp`'s caller) is a catch
//!   point: it prints in the `match` form inside the prelude's
//!   `catch_restore`; each call of the save keeps the buffer it passes and
//!   its own r4–r11 and sp (what the save stored, natively its locals), and
//!   a restore of that buffer sets them back and goes on at the call's
//!   return site with r0–r3 as the restore returned. Any other unwinding
//!   (another buffer's, a real panic) goes on up.
//!
//! The save itself prints as it is: it stores the zeroes its r4–r11 are
//! natively (`saves::context_saves`: it takes nothing), and what it stores
//! no restore reads but into its own locals.

use super::Code;
use crate::func::{Plan, exits};
use std::collections::{BTreeMap, BTreeSet};

/// The program's context saves and restores, as the lift prints them.
#[derive(Clone, Debug, Default)]
pub struct Jumps {
    /// The functions a direct call of which is a save's (`setjmp`'s): those
    /// with a save in them, and those that jump into one (newlib's Thumb
    /// entry, `bx pc`, into its ARM code).
    pub save_fns: BTreeSet<u32>,
    /// The restores a save matches, by address, with their base register.
    pub restores: BTreeMap<u32, u8>,
    /// The catch points: functions that call a save directly, with the
    /// return site of each such call.
    pub catches: BTreeMap<u32, BTreeSet<u32>>,
}

impl Jumps {
    /// The return sites of `f`'s calls of a save, if it is a catch point.
    pub fn catch(&self, f: u32) -> Option<&BTreeSet<u32>> {
        self.catches.get(&f)
    }

    /// Whether `f` has a restore in it.
    pub fn restores_in(&self, f: &crate::func::Func) -> bool {
        f.blocks
            .values()
            .flat_map(|b| &b.insns)
            .any(|(a, _)| self.restores.contains_key(a))
    }
}

/// The context saves and restores of `codes` ([`Jumps`]).
pub(super) fn find(codes: &[Code]) -> Jumps {
    let (saves, restores) = crate::saves::context_pairs(codes.iter().flat_map(|c| &c.funcs));
    if saves.is_empty() {
        return Jumps::default();
    }
    let mut save_fns: BTreeSet<u32> = codes
        .iter()
        .flat_map(|c| &c.funcs)
        .filter(|f| {
            f.blocks
                .values()
                .flat_map(|b| &b.insns)
                .any(|(a, _)| saves.contains(a))
        })
        .map(|f| f.entry)
        .collect();
    // And those that jump into one, but call nothing on the way: a
    // function's own code that doesn't return before the save.
    loop {
        let before = save_fns.len();
        for c in codes {
            for f in &c.funcs {
                if save_fns.contains(&f.entry) || !f.calls.is_empty() {
                    continue;
                }
                let into = f.blocks.values().any(|gb| {
                    exits(&c.ctx, gb).iter().any(|x| match x.plan {
                        Plan::Static(k) | Plan::Rekey(k) => {
                            !f.blocks.contains_key(&k) && save_fns.contains(&k)
                        }
                        Plan::Dynamic {
                            ret: None,
                            target: Some(k),
                            ..
                        } => !f.blocks.contains_key(&k) && save_fns.contains(&k),
                        _ => false,
                    })
                });
                if into {
                    save_fns.insert(f.entry);
                }
            }
        }
        if save_fns.len() == before {
            break;
        }
    }
    let mut catches: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for c in codes {
        for f in &c.funcs {
            for gb in f.blocks.values() {
                for x in exits(&c.ctx, gb) {
                    if let Plan::Call { target, ret } = x.plan
                        && save_fns.contains(&target)
                    {
                        catches.entry(f.entry).or_default().insert(ret);
                    }
                }
            }
        }
    }
    Jumps {
        save_fns,
        restores,
        catches,
    }
}
