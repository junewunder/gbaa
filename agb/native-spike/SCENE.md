# native-spike: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-spike.gba`. It is
the program native/agb-spike lifts by hand to `crates/native-spike`; `just
native-spike` checks the two draw the same frames for the same presses
(docs/research/native-agb-spike.md).

Mode 0, BG0 and OBJ on, 1D OBJ mapping. Colours are BGR555.

- **Frame 1** (the first, drawn while it sets up): all black. Set-up
  leaves DISPCNT at 0 and backdrop colour 0 unwritten, so the frame is
  the same however long set-up takes; it must still finish before the
  first V-blank, which is why its tables are `const` (computed at run
  time, they took longer than a frame and the display came on a frame
  late, a lag frame no native program can show).
- **From frame 2**: the backdrop `0x2108`; BG0, screen block 31, a
  32x32-tile map of three 4bpp tiles (diagonal stripes in colours 1-3, a
  box with a light top-left edge, a solid tile with one dot), a quarter
  of them flipped, in BG palettes 0 and 1 by 8x8-tile block; the map repeats
  every 256 pixels.
- **Sprite 0**, 16x16 at OBJ tiles 1-4: a ring (colour 1), a centre
  (colour 2) that steps through `0x7FFF, 0x5294, 0x318C, 0x5294` every 16
  frames by the V-blank handler's frame count, and an eye (colour 3) on
  its right, drawn flipped when it last moved left. Sprites 1-127 are
  hidden.

## Input

Read once a frame, after the V-blank wait:

- **D-pad**: moves the sprite one pixel a frame (two with **B** held) in a
  1024x1024 world, clamped to it. The camera (BG0's scroll) follows with
  a dead zone: the sprite's top-left stays within x 64-160 and y 40-104 on
  screen, and the camera stays within the world (0 to 784 across, 0 to 864
  down), so at the world's edges the sprite leaves the zone.
- **A** (on its press): the sprite's palette bank steps 0, 1, 2, 0
  (yellow, magenta, cyan ring).

At start the sprite is at (112, 72) with the camera at (0, 0): on
screen at (112, 72), facing right, yellow.
