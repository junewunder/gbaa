# A UI library for in-game menus

Research for the in-game menus june wants before more Deck work: rebinding
(controller remapping, global and per ROM), fast-forward speed, undoing a
state load, save slots, fullscreen; and, later, the Lua console with its
text buffers (the scripting console unit, `scripting/console-ui`). What
should the desktop build them on, in place of or over minifb?

Researched 2026-10-23 on quack (Fedora, KDE Plasma Wayland, 2560×1600 at
fractional scaling; gamescope installed). Spikes are in `spikes/ui/`, outside
the workspace (`just check` doesn't build them).

## Summary

- **The window layer has to change whatever draws the menus.** minifb has
  no fullscreen at all: no call to enter or leave it, so no toggle while
  playing, and on Wayland no way to ask (our `fullscreen.rs` only speaks
  X11). Its resizing stretches the buffer rather than telling us the new
  size. **winit** does all of it on X11, Wayland, macOS and Windows:
  `set_fullscreen`, real resize events in physical pixels (fractional
  scaling included), the monitor's size.
- **Moving to winit means the window lives on the main thread** (macOS
  requires it; winit asks it everywhere by default). Today the whole game
  runs on a `gbaa` thread with a 256 MB stack and minifb's window is made
  there. The split: the main thread owns the window, menus and drawing;
  the game thread keeps the emulator, the audio pacing and gilrs, and
  hands each frame over (a mutex and a wake-up). Both spikes do this. Under
  gamescope at 1280×800 the presents came 16.7 ms apart, p99 17.0 to
  17.5 ms, never over 25 ms. No hitching was added, and the hand-over
  costs one thread wake-up (well under 1 ms).
- **Two options worked in spikes, driven by a pad alone:** a menu drawn by
  hand into the 240×160 frame (winit + softbuffer, 3.9 MB, 23 s clean
  build), and egui (winit + OpenGL through glutin + egui_glow, 9.0 MB,
  45 s). The others (iced, slint, eframe on wgpu, SDL3) cost more for
  what we need, or don't build without system packages.
- **Recommendation: winit + egui on OpenGL (egui-winit + egui_glow), not
  eframe.** The menus alone could be hand-drawn, but the Lua console
  (scrolling text buffers, a typed command line, a side panel) is a real
  text UI. Hand-drawn, that means writing a text toolkit; in egui it's a
  `ScrollArea` and a `TextEdit`. One UI system for both. Pad navigation
  through egui's own focus works with a small adapter (below). The price is
  about +5 MB of binary, about +20 s on a clean build (not on incremental
  ones), and an OpenGL context.

## What we have now

`crates/desktop` (`platform.rs`): minifb 0.29 opens the window, loading
X11 or Wayland with dlopen, so the build needs no system development
packages. `place_into` scales the frame by a whole number on the CPU into
minifb's buffer, centred by `fit`. `--fullscreen` makes a borderless
window of a given size and sends the X11 EWMH request by hand
(`fullscreen.rs`). gilrs reads the pads. cpal plays the sound, and the
sound paces the emulator (minifb's own limiter is off). The game, window
and all run on one spawned thread.

## Requirements, and how each option meets them

| | minifb + hand-drawn | winit + softbuffer + hand-drawn | winit + egui (glow) | eframe (wgpu) | iced | slint | SDL3 |
|---|---|---|---|---|---|---|---|
| X11 and Wayland | yes | yes | yes | yes | yes | yes | yes |
| Fullscreen toggle | X11 only, by hand; none on Wayland, macOS menu only | yes, all four | yes | yes | yes | yes | yes |
| Resize, picture refitted | stretched by minifb | yes (spike) | yes | yes | yes | yes | yes |
| gamescope (Game Mode) | yes (today) | yes (spike, 1280×800 and 4K) | yes (spike) | likely | likely | likely | yes |
| macOS, Windows | yes | yes | yes (GL 3.2 core on macOS: deprecated, still shipped) | yes (Metal, DX12) | yes | yes | yes |
| Pad alone | ours to write | ours (spike) | egui focus + adapter (spike) | same as egui | no focus nav for buttons | key-event focus; pad mapped by us | no widgets at all |
| Mouse and keyboard | ours | ours (hover and click written, not driven in a test) | built in | built in | built in | built in | none |
| Integer scaling, no added latency | CPU, as now | CPU, as now | GPU, nearest-sampled texture | GPU | custom shader widget | image copy per frame | built in (`INTEGER_SCALE`) |
| Text console, typed line (Lua) | hard | hard | built in | built in | built in | built in | none |
| Who owns the event loop | us | us (winit's `ApplicationHandler`) | us | eframe | iced | slint | us |
| Stripped size (hello / spike) | 1.1 MB | 3.9 MB | 9.0 MB | 13.8 MB | 16.0 MB | didn't build | 3.9 MB (static) |
| Clean release build, 6 jobs | 9 s | 23 s | 45 s | 58 s | 71 s | ≥44 s, then failed | 55 s (C, cmake) |
| System packages to build | none | none | none | none | none | fontconfig headers | a C compiler and cmake |

Sizes and times are from `spikes/ui/` on quack, measured one at a time
inside the build lock (`CARGO_BUILD_JOBS=6`), after `cargo fetch`. Every
binary was stripped and includes gilrs; for comparison, today's
`gbaa-desktop` release binary is 3.0 MB unstripped. The two spikes are full
menus over a scaled frame; the other columns are hello-worlds, so they
understate.

## The spikes

Both share `spikes/ui/common.rs`. A stand-in game thread makes a moving
240×160 test picture at 59.73 Hz on its own clock (as the sound paces the
real one). It stops while the menu is open, publishes each frame and wakes
the window thread through winit's `EventLoopProxy`. A pad thread reads
gilrs and turns D-pad, A, B and Start+Select (or the Guide button) into
menu controls. `SPIKE_NAV` scripts the same controls for unattended runs.
The menu: Resume, fast-forward speed (1.5× to unlimited, default 2.5×),
save slot 1 to 4, save, load, undo load, fullscreen, quit. Both draw as
soon as a frame arrives; neither waits for vblank.

How they were driven:

- **On the desktop with a pad.** A virtual Xbox 360 pad (`cu pad`) opened
  the overlay's menu with Start+Select, moved the focus and changed the
  speed. The Guide button opens Steam's overlay on a desktop with Steam
  running, so the menu shouldn't count on it. These runs stopped as soon
  as the desktop turned out to be in use.
- **In headless gamescope** (`gamescope --backend headless -W 1280 -H 800`,
  the game sees Xwayland only, as in Game Mode), with `SPIKE_NAV` scripts
  and `gamescopectl screenshot`. Both spikes showed the menu, moved the
  focus, changed values, ran actions and quit cleanly.

| Under gamescope, 1280×800 | presents | gap p50 | p99 | max | over 25 ms | draw p50 | draw p99 |
|---|---|---|---|---|---|---|---|
| hand-drawn, windowed | 554 | 16.74 ms | 17.01 | 17.21 | 0 | 0.25 ms | 0.47 ms |
| hand-drawn, fullscreen 1280×800 | 480 | 16.74 | 18.10 | 18.45 | 0 | 0.38 | 1.6 |
| hand-drawn, fullscreen 3840×2160 | 480 | 16.72 | 18.02 | 18.30 | 0 | 2.9 | 5.7 (max 16) |
| egui, windowed | 574 | 16.73 | 17.46 | 19.68 | 0 | 0.18 | 0.91 |

Fullscreen at 1280×800 came out as 5× with 40-pixel bars left and right,
as `fit` says.

### Hand-drawn (`spikes/ui/overlay`)

winit 0.30 + softbuffer 0.4. The menu is drawn into a copy of the 240×160
frame with an 8×8 bitmap font (font8x8), then scaled by the same code as
`place_into`, so it scales with the picture and looks the same everywhere.
Navigation is exactly what we write: up and down move, left and right
change a value, A acts, B closes. It's small and has no GPU dependency.

Against it: we write every widget (lists, value pickers, scrolling,
key-capture for rebinding, any text entry). At GBA resolution an 8×8 font
gives 30×20 characters, enough for these menus but not for a script
console. The CPU scaling grows with the screen: about 3 ms a frame at 4K
(fine on the Deck's 1280×800).

### egui (`spikes/ui/egui`)

egui 0.37 with egui-winit and egui_glow, the OpenGL context made by
glutin-winit (EGL, with GLX as the fallback), swap interval 0 so vblank
isn't a second clock. The GBA frame is an egui texture sampled nearest,
placed at a whole scale in physical pixels (correct under fractional
scaling). The menu is an `egui::Window` of ordinary buttons, sliders and
a checkbox. Pad controls become arrow, Enter and Escape key events
pushed into egui's input, and egui's spatial focus does the rest: down
moves to the next widget, left and right move a focused slider, A
presses a focused button or checkbox.

What the spike taught:

- egui 0.37 ships **no fonts by default**: without the `default_fonts`
  feature every glyph is a box.
- When the game is paused nothing redraws, so egui's **repaint requests**
  have to be honoured (its layout settles a frame late otherwise: a
  click's result showed only on the next draw).
- **Escape clears egui's focus**, so B has to close the menu before egui
  sees it, and the menu has to set the first item's focus when it opens.
- A slider's value box is a focus stop of its own. Menus for a pad want
  `show_value(false)` or our own row widget (a label, `< value >`) so each
  row is one stop.
- Text needs zoom for a TV or the Deck (the spike uses 1.5×), and a dark
  console-like style. That's styling, not missing features.

Against egui: more binary and build time, and OpenGL. On macOS OpenGL is
deprecated but still shipped, and egui_wgpu (Metal) is the swap if it ever
goes. Its pad support is our adapter, not a feature egui has.

## Options not spiked, and why

- **eframe** (egui's own framework, here on wgpu). It owns the event loop
  and decides when to repaint. We want the game's frames to drive
  presents and the menu to sit over our own frame, which egui-winit does
  directly. wgpu adds Vulkan, Metal and DX12 backends and 5 MB to draw one
  textured quad.
- **iced 0.14.** It owns the loop (an Elm-style `update`/`view`), has no
  keyboard or pad focus navigation for buttons (only `focus_next`
  operations, aimed at text inputs), and needs a custom shader widget
  to show a frame at a whole scale. It's the largest and slowest here.
- **slint 1.18.** A declarative `.slint` language with key-event focus
  (`FocusScope`); a pad would be mapped to key events, as with egui. It
  owns the event loop, and the frame goes in as an image copied every
  frame. On Linux it needs fontconfig's development headers to build: it
  failed here without them, and its font crate (fontique) doesn't build in
  fontconfig's dlopen mode. Its licence is GPLv3, a royalty-free licence
  that requires attribution, or a paid one. More moving parts than egui
  for no gain we need.
- **SDL3** (the `sdl3` crate, built from source and linked statically).
  It handles the window, fullscreen and integer scaling
  (`SDL_SetRenderLogicalPresentation` with `INTEGER_SCALE`) well and is at
  home under gamescope. But it has no widgets, so the menus would still be
  hand-drawn or egui through a third-party SDL backend. It brings a C
  build (a compiler and cmake on every platform, harder cross builds) and
  its own gamepad and audio, which overlap gilrs and cpal (both to stay).
  It only replaces winit, which needs no C.
- **Staying on minifb with a hand-drawn menu.** The cheapest now, but a
  dead end for the fullscreen toggle (none on Wayland, macOS or Windows)
  and for a properly resizable window.

## Recommendation

1. **Move the window to winit**, with the game on its own thread and the
   window on the main one, as in the spikes. This is needed for the
   resizable window (`stretch/resizable-window`), the fullscreen toggle
   (`stretch/fullscreen-toggle`) and macOS (`stretch/macos`) whatever
   draws the menus, so it comes first. Keyboard input arrives as winit
   events on the main thread and goes to the game thread as held-key bits.
   gilrs and cpal stay where they are; `runtime` doesn't change, and stays
   no_std.
2. **Draw with egui through egui-winit and egui_glow**: the frame as a
   nearest-sampled texture at a whole scale, the menus as egui windows
   styled for a TV, and a small adapter turning pad controls into egui's
   arrow, Enter and Escape keys (with the fixes listed above). Later, the
   Lua console in the same toolkit.
3. If june would rather keep the binary small and stay off the GPU, the
   hand-drawn menu on winit + softbuffer does every menu on the list. The
   Lua console would then need a text toolkit of our own, or egui added
   alongside it then. The winit move in step 1 is the same either way, so
   it isn't wasted.

Either way the work splits into units: the winit move (window, resize,
fullscreen toggle, thread split), then the menu framework and its pad
adapter, then each menu (speed, slots, undo, rebinding).
