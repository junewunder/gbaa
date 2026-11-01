# The Controls page, redesigned

june (2026-10-28): the remap page isn't intuitive. Nobody found how to unbind
a control, and "press left to unbind" got "what does that mean". This is the
proposal; the mockup is `remap-mock.html` (rendered: `remap-mock.png`, four
states of one screen).

## What's wrong with the page now

- A row is `a *   lb rb` with a hidden verb on each direction: A listens and
  **replaces** every control on the row with the one pressed; Left drops the
  row's line. Neither is on screen; the footer hint names keys, not actions.
- `*` means "set where you're editing", which nobody reads.
- "Change for: this game / all games" and "Controls of: pad / keyboard" are
  rows like any other, so it isn't clear they change what the list shows.
- Binding a control another button had silently takes it from that button.
- Names are the file's (`fast-toggle`, `lstick-up`, `rthumb`).

## What good rebinding screens do

- **Steam Input / Switch remap:** a picture of the pad, each button's
  binding written on it; the list beside it is the same data.
- **Dolphin / RetroArch:** a binding is a box you click; it listens with a
  visible countdown, and a timeout cancels it. RetroArch's "hold to clear"
  is the hidden-key mistake we're fixing.
- **Modern console games (Forza, Spider-Man, Elden Ring):** a binding slot
  you focus shows its action in the button-prompt bar ("A: Remove");
  conflicts are said out loud ("LB is also on L: swap?"); "Reset to
  default" is a button, never a key.

## The design

1. **Tabs at the top: Controller | Keyboard**, the page's top row, switched
   by left and right on it or a click (june, 2026-10-29: no LB/RB or Tab). The two tabs are the same screen; only the
   chips' names differ.
2. **"Changes apply to: [Every game | Only <game title>]"**, a segmented
   switch under the tabs, with one line saying what it means. In "only this
   game", rows the game changed carry a purple **this game** tag and a
   **↺ use every-game** button; the others say nothing (june, 2026-10-29:
   *from every game* on every row was noise).
3. **A picture of the GBA** on the left, each button labelled with what
   presses it; the focused row's button lights up.
4. **Each binding is a chip with a ✕.** Clicking the ✕, or A on a focused
   chip, removes that one control. Focus moves with the D-pad: left/right
   along a row's chips, up/down between rows. When a chip has the focus its
   ✕ turns red and the prompt bar says what A does ("A remove LT from L").
   This is the unbind june couldn't find: it's on every binding.
5. **"+ Add" at the end of each row** listens. Binding now adds a control
   rather than replacing all of them (to replace: ✕ the old one, add the
   new). A row with nothing on it says in red *not bound*.
6. **Listening is a dialog**, the rest dimmed: "Adding a control to the
   GBA's B. Press a button or move a stick…", a draining 5 s bar, and a
   Cancel button. Any pad button binds (B binds B, so it can't cancel);
   Cancel is the mouse, Esc, or letting the bar run out. On the keyboard
   tab, Esc cancels and can't be bound.
7. **Conflicts are said, with undo.** A control already on another button
   moves (as now), and a toast says so: "⚠ LB moved from L to B (this game
   only). Undo (Y)".
8. **Two groups:** *Game Boy buttons* (A, B, L, R, Start, Select, the four
   directions) and *Shortcuts* with plain names: Fast-forward (hold),
   Fast-forward on/off, Save state, Load state, Rewind (hold), Script
   console. The four directions share one row whose chips are whole pads
   (D-pad, left stick, arrow keys); adding a stick binds all four ways, and
   a row whose directions differ shows them as four rows.
9. **Reset** at the foot: "Reset every game to defaults" or, in this-game
   mode, "Reset this game to every-game controls", each asking once.
10. **The prompt bar** at the foot names the action, never just a key:
    "A remove LT from L", "A add another control to B", "B back".

The file format doesn't change: the page still edits `gamepad.conf` (or
`--pad-config`) line by line, as `remap.rs` does now. New: a line can be
edited to drop one control from a row, and an undo of the last change.

## Is egui holding it back?

No. Everything above is in egui's reach: chips are buttons, the GBA picture
is painter shapes (rounded rects, circles, text, lines), the dialog is an
`egui::Modal`/`Area`, the toast an `Area`. The one weak spot is pad focus:
egui's arrow-key focus moves by geometry and is erratic across a grid of
chips of different widths. We'd keep our own focus (row, chip) for this
page and draw the focus ourselves, as the menu already turns pad presses
into its own moves. That's about a day's work, not a reason to leave egui.

## Building it

Rough size: the page and its focus model about 4 to 6 hours; the GBA picture
an hour; remove-one-control, add-instead-of-replace and undo in `remap.rs`
with tests an hour or two. The existing remap tests change with the new
verbs (A on "+ Add" adds; A on a chip removes). The check: someone who
hasn't seen it binds and unbinds a control without being told how.

## After the first bench test (june, 2026-10-29)

- The scope switch is centred under the tabs.
- The directions' together/apart is a switch, *Directions bound
  separately* (off by default), just above their rows, not a foot button.
- A focused control is outlined over the widget, not given a stroke, so
  nothing grows or shrinks as the focus moves.
- Delete and Backspace remove the focused chip on the keyboard.
- Toasts go after four seconds; undo still works after.
- The pads' tab ends with the sticks' and the triggers' deadzones (40%
  and 20% by default, 5 to 90), stepped with left and right. A switch
  over them, off by default, gives a stick's up and down a deadzone
  apart from its left and right: two rows in place of the sticks' one.
- The menu's stick moves once per push, and its spring's overshoot as
  it's let go isn't a move. A direction held repeats after 0.5 s, then
  every 0.1 s.
- Steam Input opens a virtual pad beside the Steam Controller and may
  send its D-pad as arrow keys: the same move from two places within
  80 ms is one, and so is anything just after a bind. While listening,
  only the tab's own kind of control (a pad's, or a key) counts.
- Every SDL button has a name in the file (a grip bound as `?` crashed
  it); a control the file can't name is refused, said in a toast.
- The pointer shows while the menu is up, fullscreen too.
