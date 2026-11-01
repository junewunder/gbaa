# A first real Emerald mod, and what hurt

Unit mod/agent-first-mod, 2026-10-30. An agent did, in june's place, what
a modder would: picked a small visible change, cut the function it lives in
(`docs/mod-view-design.md`, "A mod copy"), edited the readable print, built
and looked at the result. This page is what was hard, ranked, each with the
fix it suggests; each fix is filed as a mod unit (named below).

## The mod

`mods/emerald/text-speed/`: one function, `SetDefaultOptions`
(`src/new_game.c:91`, key `0x0808433d`), which a new game runs (through
`Sav2_ClearSetDefault`, from the intro when there is no save). Two edits:

- the default text speed is `OPTIONS_TEXT_SPEED_FAST` (2), not `_MID` (1);
- the default window frame is 5 (frame "Type 6"), not 0.

Steps, about 25 minutes of work:

```
just mod-cut "$ROM" ~/.local/share/gbaa/names/pokeemerald.toml mods/emerald/text-speed SetDefaultOptions
# edit mods/emerald/text-speed/src/SetDefaultOptions.rs
target/release/gbaa-recompile "$ROM" out/emerald-text-speed --names "$NAMES" --mod mods/emerald/text-speed
just build-recompiled out/emerald-text-speed      # heavy, about 4 min
target/recompiled/release/gbaa-emerald-text-speed "$ROM" --headless --frames 2001 \
    --input 'start@300,a@900,a@1100-5380/20' --dump-at 1100,2000 --dump-dir out/textmod/mod
```

The same run on the stock interpreter (`gbaa-desktop`, same presses) is
the baseline. On the new-game route's presses:

- frame 1100, the main menu: the NEW GAME and OPTION boxes draw in frame
  5's green border, not the stock blue one (the main menu loads the window
  frame from `optionsWindowFrameType`, `main_menu.c:2192`);
- frame 2000, Birch's speech: stock is at "But everyone calls me the
  POKéMON PROFESSOR."; the mod is two lines on, "This world is widely
  inhabited by cre…", with Lotad out, since each line prints faster.

Frames 1200 to 1500 (Birch's fade-in, before any text) are the same.

## What hurt, ranked

1. **Bitfield writes print as byte masks.** pret's line 93 is
   `gSaveBlock2Ptr->optionsTextSpeed = OPTIONS_TEXT_SPEED_MID;`. The print
   is a load and a store of `Ptr::<u8>::at(v7.wrapping_add(20))` with
   `(0xffff_fff8 & v11) | 1`. To change the value I had to know that
   `0xfffffff8` keeps the other five bits, so the field is bits 0-2, and for
   line 94 that the frame type is bits 3-7 and goes in as `<< 3`. The
   field accessors exist (`SaveBlock2::optionsWindowFrameType()`, whose doc
   says "bits 3-7 of the u8 at 20"), but the store never uses them: a store
   carries no mask, so `types::walk` can't pick a bitfield. Worse, line 94
   (the window frame line) reads the byte as `.optionsTextSpeed()`, because
   its mask `7 &` keeps the text-speed bits: right by the rule, misleading
   to a reader, who sees the text speed named on the line that sets the
   frame. **Fix:** recognise a load, mask, or-in and store of the same
   bitfield byte as one field write, and print it as such
   (`m.set_bits(at, p.optionsWindowFrameType(), 5)` or similar, expanding
   to the same accesses), so the edit is the value alone. Built
   (mod/bitfield-writes): `m.set_bits(lat, sat, p.optionsWindowFrameType(),
   0)`, `docs/mod-view-design.md`, "Bitfield writes"; one set from a
   computed value too (mod/computed-bitfield-writes).
2. **Values are bare numbers.** `| 1` is `OPTIONS_TEXT_SPEED_MID`; finding
   that, and that FAST is 2, meant grepping pret's `include/constants/`.
   The window frame's range (0 to 19) meant reading `text_window.c`. **Fix:**
   where a field's values are a pret `#define` family or enum, the source
   map records the family by name and value (names, not C text, as function
   names are) and the print names the value; a modder's edit can then use
   the same name. Built (mod/value-names): `[values]` from pret's
   `#define`s, linked where pret's source sets or compares the field
   with them; line 93 prints `OPTIONS_TEXT_SPEED_MID`,
   `docs/mod-view-design.md`, "Value names". Enums and the window frame's
   range (no `#define`s) are not covered.
3. **No one step from an edited mod to a frame.** `mod-states` only proves
   an unedited cut is the same; to see an edit I recompiled with `--mod`,
   built and ran the binary by hand, ran the stock build for a baseline and
   compared PNGs myself. **Fix:** `just mod-run ROM NAMES DIR [--dump-at
   N,…] ROUTE` builds the mod and dumps the route's frames from the mod and
   from the stock build, and writes a side-by-side PNG of each pair that
   differs (and says which are the same). Built (mod/mod-run):
   `route-check --mod-run`, `out/mod-run/<route>/side-N.png`.
4. **What every call's addresses mean, and what's safe to change.** Each
   access carries its instruction's address (`m.get(0x0808_4340, …)`,
   `m.get_via(0x0808_433c, 0x0808_438c, 0x0808_433e, …)`); the epilogue is
   `ret!(m, pc @ …, Exit::Exchange(v116), 54 + rt::fetch(m, 8, true, 21,
   20), { regs!(m; 0 = v104, 1 = v95, 2 = v90, …) })`. The file says
   neither what they are for (the address prices the prefetch stall,
   `codegen::emit`'s `Game` trait; the `ret!` cost is the function's cycle
   count; `regs!` is the registers the caller sees) nor what an editor
   should do with them: which address to give an access they add, whether
   a changed line must change the cycle count, what breaks if they delete
   a line whose value `regs!` names. My edit only changed constants, so
   none of it bit, but a larger edit would hit all three. **Fix:** the cut
   file's header says, in a few lines, what `at`, `pc`, the `ret!` cost and
   `regs!` are, and the rules for an edit (reuse the nearest access's
   address; the cost and `regs!` may stay as cut; an edit is not expected
   to keep exact timing), with `docs/mod-view-design.md` giving the why.
   Built (mod/cut-file-guide): every cut file's header, `codegen::modcopy`'s
   `HEAD`; the why in `docs/mod-view-design.md`, "Editing a cut".
5. **Finding the function to cut.** The change was "the default text
   speed"; finding that `SetDefaultOptions` holds it, and that it is a
   function of its own in the ROM (it is `static`, so it might have been
   inlined), meant grepping pret's source and then the source map. Also
   found only by looking: the same field draws the main menu's boxes. **Fix:**
   `just mod-find ROM NAMES FIELD` lists the functions whose readable print
   reads or writes a field (`optionsWindowFrameType`), split into writers
   and readers, by pret file:line. Done (mod/mod-find): it names
   SetDefaultOptions the one writer of `optionsTextSpeed` and four readers.

Not hurting, worth saying: the cut took 30 s and was right first time; the
`// src/new_game.c:93` comments made each pret line findable at once; the
modded crate built with no warnings from the edit, and the change showed in
the first run.
