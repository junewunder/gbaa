# The menu's skin (Aseprite)

Not read by the desktop yet: this is the template a skin loader will use.
Until then the menu draws egui's stock dark theme (`src/menu.rs`).

- `egui-dark-reference.aseprite`: the stock menu as it draws today, at 1x
  (one point a pixel), the main page in frame 1 and the Controls page in
  frame 2. Its slices mark the panel (214×261) and a focused row (200×28).
  Look, don't edit.
- `menu.aseprite`: the skin. Every piece is a named slice, drawn at 1x; the
  desktop will scale it by whole numbers only, with nearest-neighbour
  filtering. The slices start as egui's stock pieces:

| Slice | Size | What it is |
|---|---|---|
| `panel` | 24×24, 9-slice | the menu's box: fill (27,27,27), 1 px border (60,60,60), 6 px round corners |
| `row_focused` | 24×28, 9-slice | the focused row's highlight: (0,92,128), 4 px corners |
| `row_hover` | 24×28, 9-slice | a row under the mouse: (64,64,64) |
| `separator` | 8×1, stretched | the line under the heading |
| `arrow_left`, `arrow_right` | 5×7 | either side of a focused row's value (egui draws `<` and `>` as text) |
| `color_text` … `color_dim` | 6×6 swatches | text colours (normal, focused, greyed, heading) and the dimming over the game (black at alpha 140); the loader reads each swatch's colour |

A 9-slice's corners stay as drawn and its edges and centre stretch, so a
piece can be any size; set the centre in Aseprite's slice properties
(9-slices). Keep the slice names: they're what the loader looks up.

Export after editing, so the PNG and JSON match the `.aseprite`:

    aseprite -b crates/desktop/skin/menu.aseprite --sheet crates/desktop/skin/menu.png \
      --data crates/desktop/skin/menu.json --format json-array --list-slices

Text is still egui's own font; a pixel font is a later choice.
