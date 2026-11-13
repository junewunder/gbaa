# Entry reads of r4-r11: takers, context saves and broken chains

What `codegen::saves::summary` counts as a function taking one of r4-r11
from its callers, after `native/moonbeam-taker-chain`. The hole kinds are
listed in `native-holes.md`, "Entry reads of r4-r11, by the bits that
survive".

## A context save, and where a broken chain's hole is

`native/moonbeam-taker-chain`, 2026-11-13. Calls passing registers
(`native/metroid-entry-reads`) took moonbeam's hard holes from 11 to 503,
Rogue's from 21 to 48 and the corpus from 112 to 629 (at 4fb9a5cf; 46
and 626 on the main of the table below). Two roots, by
following each hard function's `which 0x… takes` to the reads that aren't
pass-ons:

- moonbeam, 499 of its 500: newlib's `setjmp` (0x0803830d, a `bx pc`
  veneer to ARM `stm r0!, {r4-r12, sp, lr}`). It stores all of r4-r11, so
  it took all eight, every function above it took what it hadn't set
  (713 functions), and each of those a pointer may call was hard.
- Rogue, 29 of its 30: 0x081b42e1 keeps a two-byte struct in r11, never
  set, and passes it to itself by value in r0 (`ands` with 0xffff00ff:
  r11's top half is the padding); the callee stores the word. A real
  use, of 57 functions' r11, 29 of them entered through a pointer.

**A context save isn't a use.** One unconditional ARM `stm` of r4-r11
with sp and lr through another register, each of r4-r11 whole and still
as it came, in a program that has the matching restore: an `ldm` of the
same register list the same way, so each word goes back into the
register it came from (`longjmp`'s `ldm r0!, {r4-r12, sp, lr}`). With sp
and lr beside them the block is a point to resume at, and r4-r11 in it
are what any callee keeps for its caller: saved, as a `push` saves
them. Everything else stored through a pointer is still a use: the same
`stm` with no restore in the program, a restore of other registers or
the other way, a list without sp and lr, a register changed before it is
stored, or bits of one stored alone (Metroid's save writer: half of r9
into a save buffer). Two limits. A Thumb `setjmp` that moves r8-r11
through low registers isn't one instruction and stays a taker. And
nothing shows that no other code reads the block's words as data (a
collector scanning a `jmp_buf` for pointers would): natively they are the
zeroes `Regs::call` gave, as a stack frame's saved registers are. (The
native check skips the stack; a `jmp_buf` in a global would differ from
the play build's there, and say so.)

**A broken chain's hole is its reader's.** A function entered through a
pointer that only passes a register on uses nothing itself; the zero is
wrong where it is read. So the read at the end of the chain is the hard
hole, once, naming the lowest function it is zeroed through and how many
more, and each pass-on is soft. One hole a reader, as before calls passed
registers, and the readers are those of f52fe115 again (Metroid
0x08073ce5, Emerald 0x080852d5, Nightmare's four). What the lift passes is unchanged (a `bl` passes, `call_fn`
doesn't, a taker's plain entry stops), so naming a chain's pointer calls
in `[[takes]]` closes it as before.

Hard holes (`just holes-all`), at f52fe115 (before calls passed
registers), on main (fd678a87) and with both changes:

| Game | f52fe115 | main | now |
|---|---|---|---|
| Metroid | 5 | 5 | 5 |
| WarioWare | 9 | 8 | 8 |
| Kirby Mirror | 3 | 1 | 1 |
| Kirby Nightmare | 11 | 14 | 10 |
| moonbeam | 11 | 503 | 4 |
| Emerald | 9 | 11 | 8 |
| Minish Cap | 27 | 24 | 24 |
| Rogue | 21 | 46 | 18 |
| Imperium | 16 | 14 | 14 |
| corpus | 112 | 626 | 92 |

moonbeam's four: three unresolved jumps and 0x08025a29 (reads r8 and r9,
zeroed through 0x08027ff9); `setjmp` takes nothing and 0x0800019b's
callers all pass r4. `longjmp` (0x08038325) has only `a jump to an
unknown place`, which the report doesn't count: a return to another
frame, `native/longjmp-uncounted`'s to look into.
