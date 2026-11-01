# Emulator UI screenshots: index

26 images in `screenshots/`. Each file was checked with `file` (all real PNG/JPEG) and viewed. Descriptions below cover only what is visible in the image. Version numbers come from title bars or captions in the image where shown.

Source quality, best first: the project's own website or GitHub README/release notes, then the project's app-store or Flathub listing (screenshots the developers submitted), then Wikimedia Commons uploads.

## OpenEmu (macOS)

| File | Screen | Source |
|---|---|---|
| `openemu_library_grid.png` | Library, NES console selected, cover grid | [openemu.org](https://openemu.org/img/intro-nes-grid.png) |
| `openemu_library_md.png` | Sonic 2 running in a window over the library, with the in-game HUD bar | [openemu.org](https://openemu.org/img/intro-md.png) (also used in the [OpenEmu README](https://github.com/OpenEmu/OpenEmu)) |
| `openemu_controls_prefs.png` | Preferences > Controls, NES pad picture | [openemu.org](https://openemu.org/img/controls-prefs.png) |
| `openemu_savestate_name_dialog.png` | "Save As" sheet for naming a manual save state | [OpenEmu wiki, "Next Version" page](https://github.com/OpenEmu/OpenEmu/wiki/Next-Version) (image: http://i.imgur.com/ikZWXCs.png) |
| `openemu_setup_assistant.png` | First-run setup assistant ("Welcome to OpenEmu!") | [OpenEmu wiki, User guide: Getting Started](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Getting-Started) (image: http://i.imgur.com/6Z8GKBi.png) |

- **Library grid:** a dark window in the iTunes style. A source-list sidebar on the left lists about 25 consoles, with Collections below them (All Games, Recently Added, 8-bit Classics, My Favorites). A segmented control at the top switches between **Library / Save States / Screenshots / Homebrew**, next to a cover-size slider and a search field. Big box-art tiles have the title and a 5-star rating under each. Few words and no settings anywhere on screen; the box art does the work.
- **In-game HUD:** the game plays in its own borderless window. A translucent rounded bar floats over the bottom of the picture with about 7 icon-only controls (power, pause, restart, a save/options button, settings gear, volume slider, fullscreen). The library stays visible behind the window.
- **Controls prefs:** this is the screen you asked about with the console's controller picture. A large illustration of the real pad sits on a wood-grain background. On the right are a console dropdown, a "Player 1" dropdown, a "Gameplay Buttons" list (Up, Down, Left, Right, Start, Select, A, B) with key fields, and an Input dropdown (Keyboard/gamepad). A toolbar along the top holds the tabs Library, Gameplay, Controls, Cores, System Files. It is skeuomorphic and shows only one console's buttons at a time.
- **Save-state dialog:** a small dark sheet with a text field pre-filled as `Save-Game-3 2014-09-14 09:40:23 +0000`, a "Do not ask me again" checkbox, and Cancel / Save Game. Saving is one step, with an optional name.
- **Setup assistant:** a full-window first-run wizard over a retro synthwave grid background, with short paragraphs and Back/Next. Another image on the same wiki page shows its core-picker step.

## mGBA (Qt desktop frontend, 0.10.0)

| File | Screen | Source |
|---|---|---|
| `mgba_qt_main.png` | Main window, no game loaded | [Flathub io.mgba.mGBA](https://flathub.org/apps/io.mgba.mGBA) (caption "mGBA main window") |
| `mgba_qt_ingame.png` | Pokémon Emerald title screen running | same (caption "Game window") |
| `mgba_qt_settings.png` | Settings dialog, Audio/Video page | same (caption "Settings window") |

- **Main / in-game:** a plain native window. The menu bar has File, Emulation, Audio/Video, Tools, and the game picture fills the rest. There is no library, toolbar or HUD. The title bar shows the game name and live FPS ("POKEMON EMER (60.2 fps)"). When no game is loaded, the window shows a 3D GBA graphic with the mGBA logo.
- **Settings:** a dense desktop dialog. A left list holds 14 categories (Audio/Video, Gameplay, Interface, Update, Emulation, Enhancements, BIOS, Paths, Logging, Game Boy, Keyboard, Controllers, Shortcuts, Shaders). The Audio/Video page alone has about 15 controls: driver dropdowns, buffer and sample-rate fields, volume sliders, radio buttons, and checkboxes such as "Lock aspect ratio" and "Force integer scaling". Apply/Cancel/OK sit at the bottom. It is built for mouse and keyboard, not a pad.

## RetroArch

| File | Screen | Source |
|---|---|---|
| `retroarch_xmb_main.jpg` | XMB menu driver, Main Menu (v1.7.1) | [libretro docs: XMB guide](https://docs.libretro.com/guides/xmb/) |
| `retroarch_xmb_thumbnails.png` | XMB playlist (Atari Jaguar) with box-art thumbnail (v1.18.0) | same page |
| `retroarch_ozone_main.png` | Ozone menu driver, Main Menu (v1.9.2) | [Wikimedia Commons: Retroarch screenshot.png](https://commons.wikimedia.org/wiki/File:Retroarch_screenshot.png) |
| `retroarch_4_guis.jpg` | The four menu drivers side by side (Ozone, XMB, RGUI, GLUI/MaterialUI) | [libretro docs: The User Interface](https://docs.libretro.com/guides/gui/) |

- **XMB:** a PS3/PSP-style cross bar. A horizontal row of category icons runs across a blue wave background, with a vertical list of items under the selected icon. Each item has an icon and a one-line description. In the playlist view the selected game's box art shows on the right. It is pad-first and shows few items at once.
- **Ozone:** a Switch-like two-pane layout. A left sidebar has about 10 sections (Main Menu, Settings, Favorites, History, Images, Music, Videos, Netplay, Import Content), and the right list holds items with grey help text under each. Button hints sit in the footer (Search / Back / OK), and the clock and battery are at the top right. Pad-friendly but fairly dense.
- **Four drivers:** this shows that the same menu tree is skinned four ways, including RGUI's low-res green-on-black text menu and the touch-oriented GLUI.
- **Gap:** I found no official image of the in-game **Quick Menu** (the pause menu with Resume, Save State, Load State and so on). The libretro Quick Menu page ([docs](https://docs.libretro.com/guides/quick-menu/)) has no screenshot of it, and the only image near it was a web-player toolbar button, which I deleted.

## Delta (iOS, 2.0)

| File | Screen | Source |
|---|---|---|
| `delta_gba_skin.png` | GBA game on the default purple Delta controller skin, portrait | [App Store listing](https://apps.apple.com/us/app/delta-game-emulator/id1048524688) |
| `delta_pause_menu.jpg` | In-game pause menu | same |
| `delta_library.jpg` | Game library ("Favorites" page) | same |
| `delta_gbc_gba_skins.png` | Promo: a GBC skin in portrait and a GBA skin in landscape | [Delta README](https://github.com/rileytestut/Delta) |

- **Skin:** the screen takes the top part of the phone. The bottom is a full-width picture of a GBA-styled controller (D-pad, A/B, L/R tabs, Select/Start, a green Menu button). The skin is the whole input layer, and its look copies the real console.
- **Pause menu:** a 2x3 grid of large rounded frosted-glass buttons, each an icon plus label (Save State, Load State, Cheat Codes, Fast Forward, Hold Buttons, Screenshot), over a blurred live game frame. There is a Home button at the bottom left and a big purple Resume (play) button at the bottom right. Six options only, sized for touch.
- **Library:** a 3-column grid of box art with titles over a blurred tinted background. There are paging dots, a search field at the bottom, and settings, play (resume) and + (import) buttons in the header. It looks like an iOS home screen.

## DuckStation (PS1)

| File | Screen | Source |
|---|---|---|
| `duckstation_qt_gamelist.png` | Qt main window, game list (list mode, v0.1-10056) | [DuckStation README](https://github.com/stenzek/duckstation) (raw.githubusercontent.com/stenzek/duckstation/md-images/main-qt.png) |
| `duckstation_bigpicture.png` | Big Picture (fullscreen) home | same README (md-images/bigduck.png) |

- **Qt game list:** a dark table with columns Icon, Serial, Title, Year, Time Played, Last Played, Region flag, Achievements (e.g. "1/104"). A toolbar has list/grid toggles, a zoom slider, type and region filters, and search. It is built for collectors and very dense, with a dozen rows at desktop size.
- **Big Picture:** four big tiles centred on a dark gradient (Game List, Start Game, Settings, Exit), each with a short description. A footer shows controller hints (About, Resume Last Session, Toggle Fullscreen, Navigate, Select, Exit), and the user avatar and clock are at the top right. Fully pad-first. It is a separate UI layer from the Qt window.

## Dolphin (GameCube/Wii, 5.0-3158)

| File | Screen | Source |
|---|---|---|
| `dolphin_gamelist.png` | Main window with game list | [Flathub org.DolphinEmu.dolphin-emu](https://flathub.org/apps/org.DolphinEmu.dolphin-emu) ("Dolphin's main window") |
| `dolphin_graphics_settings.png` | Graphics Configuration, General tab | same ("Graphics configuration") |
| `dolphin_controller_config.png` | GameCube Controller Configuration, Port 1 | same ("Controller configuration") |

- **Game list:** the menu bar, then a big-icon toolbar (Open, Refresh, Play, Stop, FullScr, ScrShot, Config, Graphics, Controllers), then a light table with Banner, Title, Maker, Size, and State as a star rating. A plain desktop look, and this is a fairly old build.
- **Graphics config:** tabs (General / Enhancements / Hacks / Advanced) with grouped dropdowns and checkboxes. A "Description" box at the bottom explains whichever option the mouse is over, which is a useful idea to borrow for help text.
- **Controller config:** one dense window grouped into Buttons, Control Stick, C Stick, Triggers and D-Pad, with a small stick-position preview for each stick, threshold, radius and dead-zone spinners, and profile Load/Save/Delete. It uses text binding fields, not a controller picture, which is the opposite of OpenEmu's approach.

## SkyEmu (GB/GBC/GBA/NDS, ImGui UI)

| File | Screen | Source |
|---|---|---|
| `skyemu_v4_desktop_mobile.jpg` | v4 release banner: a desktop window and a phone running Varooom 3D | [SkyEmu v4 release notes](https://github.com/skylersaleh/SkyEmu/releases/tag/v4) |

- The desktop window has a slim icon toolbar along the top (play, pause, step and so on) and a dark left side panel with a scrolling list of entries (it looks like a menu/achievement-style list, but too small to read). The game sits in the centre. On the phone, on-screen touch controls are drawn under the game. The UI is drawn inside the app (ImGui), not with native widgets, and the same UI covers desktop and mobile.
- **Gap:** this is a marketing composite at small scale. I found no full-resolution official shot of SkyEmu's settings or menus. The README hero image (github.com/skylersaleh/SkyEmu/assets/7118296/03d74d15…) is a similar banner, and the v2 release image is a shader demo, so I left both out.

## VisualBoyAdvance-M (2.1.8)

| File | Screen | Source |
|---|---|---|
| `vbam_ingame_gba.png` | Mega Man Battle Network title screen in the wxWidgets window | [Flathub com.vba_m.visualboyadvance-m](https://flathub.org/apps/com.vba_m.visualboyadvance-m) |

- A native GTK window with a menu bar (File, Emulation, Options, Tools, Help), the ROM filename in the title bar, and the game filling the rest. There is no library or HUD, and everything lives in menus. In look it is almost identical to mGBA's main window.
- **Gap:** I found no official image of the settings or input dialogs. The other Flathub images are only more game screens, and the vba-m website's only screenshot is a photo of a phone, which I deleted.

## ES-DE (frontend, not an emulator)

| File | Screen | Source |
|---|---|---|
| `esde_01.png` | System carousel (Atari Jaguar selected, "77 Games") | [es-de.org](https://es-de.org/) |
| `esde_02.png` | Gamelist with metadata panel (ScummVM) | same |
| `esde_themes_03.jpg` | Third-party theme: grid of system tiles with year and logo | same |

- **System carousel:** a horizontal carousel of console photos with a large centre item, its logo and year, and the game count. The footer has button hints (Menu, Select, Screensaver, Choose). A 10-foot, pad-first UI.
- **Gamelist:** a text list of games on the left. The right shows box art, a screenshot, a description, and metadata (rating, release date, players, genre, developer, last played, play time). The footer has about 8 button hints. Dense, but laid out for a TV.
- **Theme grid:** a colourful tile grid with era branding on each tile. The theme system can change the layout completely, not just the colours.

## Missing (searched, nothing official found)

- **OpenEmu save-states browser** (the grid of save-state thumbnails behind the "Save States" tab). The tab shows in `openemu_library_grid.png`, but I found no image of the browser itself on openemu.org, in the README, or on the wiki (its "User guide: Save states" page has no images).
- **NanoBoyAdvance:** the README (now on [Codeberg](https://codeberg.org/nba-emu/NanoBoyAdvance)) has no screenshots, the repo holds only the app icon, and nba-emu.github.io returns "Site not found".
- **Nintendo Switch Online GBA app** (rewind/suspend menu): Nintendo's ["How to Create a Suspend Point"](https://www.nintendo.com/au/support/articles/how-to-create-a-suspend-point/) page only has game key art, which I deleted. [MarioWiki's GBA NSO page](https://mariowiki.com/Game_Boy_Advance_-_Nintendo_Switch_Online) has only button glyphs and box art. I didn't search press sites, so a third-party capture may still exist.
- **RetroArch Quick Menu** and **mGBA / VBA-M controller-mapping dialogs:** no official images found (see the notes above).

## Patterns that show across the set

- **Two kinds of UI.** Desktop-native tools (mGBA, VBA-M, Dolphin, DuckStation's Qt window) have menu bars, dense dialogs and tables. Pad or 10-foot UIs (RetroArch, DuckStation Big Picture, ES-DE, Delta) have big tiles, few items per screen, and a footer of button hints. OpenEmu sits between the two: a desktop app built around box art, with almost no visible settings.
- **Where the pause/in-game menu lives.** OpenEmu uses a floating translucent bar over the game. Delta uses a grid of six large buttons over a blurred frame. RetroArch uses a full menu (Quick Menu). mGBA and VBA-M have no in-game overlay, only the menu bar.
- **Controller mapping.** OpenEmu draws a picture of the console's own pad next to a short list of buttons. Dolphin uses a grid of text binding fields with dead-zone and threshold controls. For a GBA-only frontend there are just 10 buttons, so OpenEmu's picture approach fits without crowding.
