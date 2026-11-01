# OpenEmu and polished emulator frontends: what produces the "just works" feeling

Scope note: about 20 tool calls. Primary sources reached: openemu.org, the OpenEmu GitHub wiki (Save states, Controls preferences, Importing), the OpenEmu GitHub releases API, Riley Testut's Delta sync post, Analogue's own announcement (via press), Nintendo's NSO pages (via press and fan wiki), DuckStation's setup-wizard source, and the Dolphin bug tracker and forums. Several search results came from SEO or AI-written sites (tech-insider.org, prismnews.com, grokipedia). Those are flagged wherever they are used, and their dates contradict each other. I found no developer interview or design-philosophy blog post by the OpenEmu team. That is listed as a gap, not filled in from memory.

## OpenEmu specifics: library, import, cores, controls, save states, HUD, settings, native look, and the developers' stated philosophy

### Takeaway
OpenEmu's "just works" claim is its own slogan. In practice it comes from a few concrete choices: one library for every console, drag-and-drop import that sorts each file into the right console, automatic box art, controllers that map themselves from a built-in database, automatic save states with a "resume?" prompt on next launch, one quick-save slot plus named states you can browse, and power-user options pushed into a hidden "Secrets" preferences pane. The price is that OpenEmu owns your files (it copies them into its own folder, and you must not delete games by hand) and that some advanced options are hidden or missing.

### Cited Findings
**Stated philosophy**
- openemu.org says it extends an "'It just works' philosophy" to open-source emulation on the Mac, and its tagline is "OpenEmu is about to change the world of video game emulation. One console at a time..." — [openemu.org](https://openemu.org/)
- The project's stated aim is to bring emulation to OS X "as a first class citizen, leveraging modern OS X technologies such as Cocoa, Core Animation and Quartz" — [GitHub README (old fork)](https://github.com/clobber/OpenEmu)
- The cores are modular, so outside developers can add a core "without the need to account for specific macOS APIs". The frontend owns everything the user sees, and cores are plugins behind it — [Wikipedia: OpenEmu](https://en.wikipedia.org/wiki/OpenEmu)
- A 1.0-era press quote credits it with bringing "the idea of an emulator for a mainstream, general audience to reality". The 1.0 coverage praised its UI and ease of use, and it had over 10 million downloads by August 2018 — [Wikipedia: OpenEmu](https://en.wikipedia.org/wiki/OpenEmu)
- A MacRumors forum post (user opinion) argues that OpenEmu is built to *manage a game library*, not just play games, and that a player-focused app would suit some users better. This is the library-first versus player-first tension — [MacRumors forum](https://forums.macrumors.com/threads/openemu-lite-just-works-emulator-for-2d-console-games-on-os-x-10-7-10-9.2333375/)

**Library and import**
- "Organizes games into one unified library". Custom collections can mix games from different consoles. You drag a ROM from the desktop and OpenEmu "scans it and files it in the correct library", adding original box art automatically — [openemu.org](https://openemu.org/); the same wording appears in [SourceForge mirror](https://sourceforge.net/mirror/openemu/)
- The import wiki says that when you drag a supported file type into the window, it imports automatically. System detection goes by file extension. An unrecognised file gives "No valid system detected", which also happens if that system has been hidden. Ambiguous `.bin` files go to an **Import Issue Resolver** in the bottom-left corner of the Library window, where you pick the system by hand. A zip may hold only one ROM, and not for CD systems. A file that is already in the library is rejected with "Already in library" — [OpenEmu wiki: Importing](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Importing)
- OpenEmu owns the library: "DO NOT manually delete games from your Game Library folder"; games have to be removed inside the app — [OpenEmu wiki: Importing](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Importing)
- By default it **copies** ROM files into its Game Library folder (so you end up with duplicates on disk) and looks up game information and artwork automatically. Both can be turned off in Library preferences ("Copy files to Game Library folder", "Automatically lookup game information & artwork") — [Retro Game Corps box-art guide (2020, updated 2025)](https://retrogamecorps.com/2020/08/20/guide-scrape-rom-box-art-for-macos/)
- The cover art and game data credit goes to OpenVGDB — [GNOME Games metadata design wiki](https://wiki.gnome.org/Design/Playground/Games/Metadata). An `openvgdb.sqlite` file lives in OpenEmu's Application Support folder, so the database ships locally — [greggant.com 2015 post](https://blog.greggant.com/posts/2015/12/31/openemu-201-import-fix.html)

**Controllers**
- Generic HID USB and Bluetooth controllers "should work". Recognised devices are mapped automatically from a built-in database, configured in Controller Preferences — [openemu.org](https://openemu.org/)
- The wiki says "OpenEmu makes it simple to map controls." You pick a controller in the Input dropdown, and a supported one is mapped automatically. To map by hand you select a control and "press the button on the controller to which you would like to map it" — [OpenEmu wiki: Preferences: Controls](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Preferences:-Controls)
- A "Special Keys" section maps actions beyond the original console's buttons: save, load, quick save, fullscreen, mute, volume, reset, pause, fast forward (hold only), display mode and screenshot. "Some of the above are hidden by default and must be manually enabled in the Secret preferences." — [OpenEmu wiki: Preferences: Controls](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Preferences:-Controls)
- Supported pads include DualShock 3/4, Xbox controllers and the Wiimote — [Wikipedia](https://en.wikipedia.org/wiki/OpenEmu); [Downloadcrew 2.0.1 review](https://www.downloadcrew.com/article/31890/openemu)
- The latest upstream commits (Oct 2025) include "fix_change_controller_animation", which suggests the controller preferences pane animates between console controller pictures — [GitHub API commits, OpenEmu/OpenEmu](https://github.com/OpenEmu/OpenEmu/commits)

**Save states**
- "OpenEmu automatically saves your progress as you play a game". On the next launch "you will have the option of restarting from the automatic save state or not". If that prompt has been suppressed, you bring it back by resetting dialog warnings in Preferences: Library — [OpenEmu wiki: Save states](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)
- There is **one** Quick Save slot: ⌘S to save and ⌘L to load, under the Controls menu, or bound under Special Keys. Once it exists it can also be loaded from the HUD's disk menu — [OpenEmu wiki: Save states](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)
- Manual states: move the mouse to show the **HUD bar**, click the floppy-disk icon, choose "Save Current Game", and give it a name (the default is the date and time). To load, right-click a game in the Library and choose "Play Save Games", or use the HUD's disk menu. To delete, right-click in the **Save States collection** — [OpenEmu wiki: Save states](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)
- States are tied to the core that made them. Loading a state from another core makes OpenEmu **switch cores automatically**, which is the cores-are-invisible idea in practice. Compatibility across updates is not guaranteed — [OpenEmu wiki: Save states](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)
- The 2.0 release notes, reposted on a forum, list "Save States and Screenshots organizers" and "real-time rewinding". They also warn that "control mappings have been reset due to changes in the input system" — [search snippet of NeoGAF 2.0 thread](https://www.neogaf.com/threads/openemu-2-0-released-with-16-new-systems-requires-mac-os-x-10-11-or-above.1161929/) (the page returned 403, so this comes from the search-result snippet only)
- Feature list: full save-state support including automatic states, several ROMs playable at once, ROM-hack support, a Metal frontend, graphic filters, fullscreen — [Wikipedia](https://en.wikipedia.org/wiki/OpenEmu)

**Settings surface**
- The wiki lists only a handful of preference panes (Library, Gameplay, Controls, Cores, and a hidden **Secrets** pane) and a separate Display Mode page — [OpenEmu wiki sidebar, via Save states page](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)
- The default core for each system is a setting ("User Guide: Default Cores"), but cores are otherwise not exposed in the main flow — same sidebar

### Inferences
- The pattern behind OpenEmu's ease is **progressive disclosure taken to the extreme**: sensible defaults; one quick slot instead of 10; extra hotkeys hidden in a "Secrets" pane; cores switched silently. For gbaa there is only one "core" (the recompiled game), so that part is solved for free. The lesson to carry over is to keep the visible settings tiny and put power-user switches behind a deliberately hidden pane.
- The **auto-save plus "resume?" prompt** is the single feature most directly responsible for the "I quit and nothing was lost" feeling. With "don't ask again" it can become a silent resume. It maps cleanly onto a per-game gbaa executable: save a state on window close, offer to resume on launch.
- A named save-state list with a date-and-time default name means the user never has to type a name. OpenEmu's Save States collection (with screenshots, per the 2.0 notes) turns states into browsable objects, not numbered slots.
- I could not confirm from a primary source in this pass that the controller pane shows a picture of each console's controller. The Oct 2025 "controller animation" commit is indirect evidence only. Treat it as likely but unverified.

### Gaps
- No OpenEmu developer interview, blog post or talk on design philosophy turned up (searches for team interviews returned only aggregators). The openemu.org slogan is the only primary philosophy statement found.
- Not verified from a primary source: the exact default shader or filter, the HUD's full contents (volume, filters, fullscreen, pause), whether states are saved on window close specifically (the wiki only says "as you play"), and window restoration behaviour.
- Whether OpenVGDB is still maintained: no source found.

## Criticisms and weaknesses of OpenEmu

### Takeaway
The verifiable criticisms are slow, irregular releases (a three-year gap from 2.3.3 in January 2021 to 2.4 in December 2023, and nothing tagged since), an official build that is still Intel-only (a native Apple Silicon build exists only as a community fork, OpenEmu-Silicon), heavy-handed library ownership (copied files, no manual deletion, duplicate rejection, `.bin` ambiguity), and gaps per core (GameCube has no save states). The claim that "OpenEmu has no rewind" is **false**: rewind arrived in 2.0.

### Cited Findings
- Release cadence from the GitHub API: 1.0 (2013-12-23), 2.0 (2015-12-24), 2.0.x through 2019-06, 2.1 (2019-10-25), 2.2 (2019-12-27), 2.2.1 (2020-01-25), 2.3 (2020-12-23), 2.3.1–2.3.3 (Jan 2021), then **nothing until 2.4 and 2.4.1 (both 2023-12-30)**. No release has been tagged since, although commits continued until at least 2025-10-22. The repo has 434 open issues and about 17.9k stars — [GitHub API: OpenEmu releases](https://github.com/OpenEmu/OpenEmu/releases)
- The 2.4.1 release body says in full: "Binaries are Intel-only." Each release ships a normal zip and an "-experimental" zip — [GitHub release v2.4.1](https://github.com/OpenEmu/OpenEmu/releases/tag/v2.4.1); openemu.org's download is labelled "Experimental" — [openemu.org](https://openemu.org/)
- OpenEmu-Silicon, a community fork, "runs natively on Apple Silicon (no Rosetta required)", on macOS 11+, and was tested on Sequoia and macOS 26 Tahoe — [GitHub: nickybmon/OpenEmu-Silicon](https://github.com/nickybmon/OpenEmu-Silicon). RetroAchievements announced Hardcore-mode support in OpenEmu-Silicon in June 2026 — [RetroAchievements forum](https://retroachievements.org/forums/topic/36439). The fork's version number differs between sources (1.1.2 / 1.2.2 / 1.4.0).
- A guide says the Rosetta build is slower and more prone to input hiccups than the native fork. This comes from an SEO or AI-written source and is not a benchmark — [tech-insider.org](https://tech-insider.org/how-to-set-up-openemu-pokemon-mac-2026/). Its claim that "2.4 shipped September 25, 2026" contradicts the GitHub API (2023-12-30), so that source is unreliable on dates.
- An AI-labelled 2026 article lists RetroArch features (rewind, netplay, run-ahead) that it says OpenEmu lacks, and says RetroArch now leads on Apple Silicon. Rewind, at least, is contradicted by the OpenEmu 2.0 notes ("real-time rewinding") — [prismnews.com](https://www.prismnews.com/hobbies/retro-game-emulation/mac-emulation-is-thriving-on-apple-silicon-retroarch-leads); contradicted by [NeoGAF 2.0 notes snippet](https://www.neogaf.com/threads/openemu-2-0-released-with-16-new-systems-requires-mac-os-x-10-11-or-above.1161929/)
- Fast forward is hold-only, and some of its hotkeys are hidden until you turn them on in Secrets — [OpenEmu wiki: Controls](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Preferences:-Controls)
- Feature gaps per core: Sega 32X-CD unsupported, GameCube without save states, 22 multi-disc GameCube titles unsupported — [Wikipedia](https://en.wikipedia.org/wiki/OpenEmu)
- 2.0 reset users' control mappings because of input-system changes, a breaking migration — [NeoGAF 2.0 notes snippet](https://www.neogaf.com/threads/openemu-2-0-released-with-16-new-systems-requires-mac-os-x-10-11-or-above.1161929/)
- Library friction: by default files are copied (duplicates on disk), people turn off auto artwork lookup to scrape their own, games must not be deleted by hand, `.bin` files need manual resolution, multi-file zips fail — [Retro Game Corps](https://retrogamecorps.com/2020/08/20/guide-scrape-rom-box-art-for-macos/); [OpenEmu wiki: Importing](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Importing)
- 2.0 required El Capitan and dropped Lion through Yosemite. Major releases have repeatedly raised the minimum OS — [Wikipedia](https://en.wikipedia.org/wiki/OpenEmu)

### Inferences
- OpenEmu's weaknesses mostly come from being a **multi-system library manager** that relies on volunteer upstream cores. gbaa avoids the core and BIOS matrix entirely, so it inherits the strengths without most of the weaknesses. The risk worth avoiding is the library owning the user's files.
- The long silent gaps between releases eroded trust more than any missing feature. A small project's equivalent is making sure each per-game executable never needs an update to keep working (its saves are portable, it has no online dependency).

### Gaps
- I found no Reddit, r/emulation or r/mac threads with direct quotes. Searches returned SEO pages instead. The complaint themes above come from documentation and releases, not user-sentiment data.
- Whether the "experimental" zip in 2.4.x is Apple Silicon native was not confirmed (2.4.1 says binaries are Intel-only, which suggests both zips are).

## Delta, Nintendo Switch Online and Analogue OS: minimalist choices

### Takeaway
All three hide the emulator behind a handful of verbs: suspend or quick-save, load, rewind (NSO), fast forward (Delta), and sync (Delta). Each bounds the feature deliberately: NSO's rewind is short and tied to suspend points; Analogue keeps states as separate files with thumbnails; Delta defers to the user on sync conflicts rather than risk losing data.

### Cited Findings
**Delta (Riley Testut)**
- Save and load states from the **pause menu**. States can be **locked** so they are not overwritten by accident. Delta **auto-creates save states periodically** as well as manual ones — [GitHub: rileytestut/Delta](https://github.com/rileytestut/delta); [AppleInsider guide](https://appleinsider.com/articles/24/04/19/everything-you-need-to-know-about-emulating-retro-games-with-delta-for-iphone)
- Fast-forward speed is set per game by long-pressing Fast Forward in the pause menu. A reviewer warns that speedup makes timing-sensitive sections harder — [AppleInsider](https://appleinsider.com/articles/24/04/19/everything-you-need-to-know-about-emulating-retro-games-with-delta-for-iphone); [rottenwifi review](https://blog.rottenwifi.com/hands-on-with-delta-the-app-store-emulators-current-features-setup-and-limits/). In 2021 the DS core could only fast forward to 1.5x for a few minutes before iOS throttled it — [Delta 1.3 post](https://rileytestut.com/blog/2021/04/21/delta-1-3-ds-for-everyone/)
- Skins: built-in controller skins plus importable or community-made `.deltaskin` files — [GitHub: Delta](https://github.com/rileytestut/delta)
- Delta Sync covers games, saves, save states, cheats, skins and controller mappings via Google Drive or Dropbox, with version history — [AppleInsider](https://appleinsider.com/articles/24/04/19/everything-you-need-to-know-about-emulating-retro-games-with-delta-for-iphone)
- Testut's sync design: each item is a "Record" that bundles files (for example a save state's thumbnail plus its data). ROMs sync by default ("which I do believe is the better default"). iCloud was rejected because Apple could switch it off for Delta. "Delta will defer as much as possible to the user for decisions when it is not 100% sure what the right answer is", which means more "Resolve Conflict" screens. Data integrity comes first, the app itself stays largely unaware of syncing, and in four years no GBA4iOS user reported data loss from sync — [rileytestut.com: Delta Features – Syncing (2018)](https://rileytestut.com/blog/2018/02/25/delta-features-syncing/)
- Delta reached the US App Store in April 2024 — [9to5Mac](https://9to5mac.com/2024/04/17/riley-testut-launches-delta-game-emulator-on-app-store-for-everyone-altstore-marketplace-for-eu/)

**Nintendo Switch Online (NES/SNES/GB/GBA "Nintendo Classics")**
- The GBA app's features are Suspend Points and Rewind, plus local communication, online play and customisable buttons — [Nintendo Classics page](https://www.nintendo.com/my/nintendo-switch-online/nintendo-classics/index.html); [Deku Deals listing](https://www.dekudeals.com/items/game-boy-advance-nintendo-switch-online)
- The suspend menu opens with **ZL+ZR**, and **holding** the same chord triggers Rewind. One input covers both, so the player does not have to learn a separate rewind button — [Super Mario Wiki: GBA – Nintendo Classics](https://www.mariowiki.com/Game_Boy_Advance_-_Nintendo_Classics)
- The Game Boy app allows up to **4 suspend points per game** and **up to 30 s of rewind** (this figure is for the GB app; I found none for the GBA app) — per [search summary of Nintendo/fan-wiki pages](https://nintendo.fandom.com/wiki/Game_Boy_-_Nintendo_Classics)
- NES/SNES rewind works from rolling save states captured a few seconds apart, and it is limited to rewinding from the last suspend point. The SNES Classic allowed about 1 minute (action) to 5 minutes (RPGs). Some commentators called rewind "a cheat" — [GameRevolution](https://www.gamerevolution.com/news/565055-nes-online-rewind-update-feature-save-states); [HotHardware](https://amp.hothardware.com/news/nintendo-switch-online-game-rewind-some-call-cheat)

**Analogue Pocket (Analogue OS)**
- OS 1.1 beta added a **Library** (a reference database that logs each cartridge played, with date added and **total play time**) and **Memories** (save states) — [Analogue announcement](https://www.analogue.co/announcements/os-v1-1-available-now-what-s-new-in-analogue-os-v1-1-beta); [Gizmodo](https://gizmodo.com/analogue-pocket-os-v1-1-beta-release-memories-library-1849345505)
- Memories gives 128 slots shared across GB/GBC/GBA/Game Gear. Each save is a separate file managed individually (you cannot keep overwriting one slot), and to load a state for the inserted cartridge you have to start the game first — [Engadget](https://www.engadget.com/analogue-pocket-os-retro-gaming-162028559.html) (via search summary)
- A later beta added screenshots and **thumbnails for save states** "making it easier to identify which one is which" — [Time Extension](https://timeextension.com/news/2022/11/analogue-pocket-firmware-update-adds-some-welcome-features)

### Inferences
- Common minimalist primitives: **one chord opens the menu and holding it rewinds** (NSO); **auto states plus lockable manual states** (Delta); **thumbnail on every state** (Delta, Analogue, OpenEmu 2.0); **play time per game** (Analogue). Any of these fits an egui in-game overlay without a library app.
- Delta's sync rule ("ask when not 100% sure, never lose data") is the right stance for any gbaa cloud or folder sync of `.sav` files.

### Gaps
- GBA-app rewind length and number of suspend points: not found.
- Delta's quick save/load hotkeys and rewind UI (Delta 1.5+ has rewind, from memory) were not confirmed in this pass.

## DuckStation, Dolphin and Ryujinx: game lists, per-game settings, resume prompts, achievements, fullscreen UIs, setup wizards

### Takeaway
The PC emulators make the opposite trade from OpenEmu. They expose everything, then add a **first-run wizard** and a **controller-navigable fullscreen ("Big Picture") UI** on top for the couch case. Per-game settings sit behind right-click → Properties on the game list.

### Cited Findings
- DuckStation's Qt setup wizard has these pages: BIOS Image → Game Directories → Controller Setup → Graphics Setup → RetroAchievements → Complete. Its welcome text says it guides the user through the configuration steps needed to use the app — [DuckStation setupwizarddialog.ui (mirror)](https://git.morozoff.pro/aiden/duckstation/raw/commit/e3c4f77a05070b1e6afd073e8a11726805f1fa76/src/duckstation-qt/setupwizarddialog.ui)
- DuckStation has a fullscreen or "Big Picture" UI for TVs that can be driven entirely with a gamepad, plus RetroAchievements integration — [DuckStation README (fork mirror)](https://github.com/vvsx87/duckstation); [README mirror](https://repo.retrodeck.net/Xargon/Duckstation/blame/commit/4df6f3bc4dd6c56d6971272ab559557a5e808d85/README.md)
- External launchers that try to resume DuckStation can break it. A LaunchBox user got a black screen after resuming from BigBox's pause menu and fixed it with an AutoHotkey refocus-and-fullscreen-toggle script, which shows the friction of a separate launcher driving an emulator — [LaunchBox forum](https://forums.launchbox-app.com/topic/84093-duckstation-gives-black-screen-once-resume-via-bigbox-pause-menu/)
- Dolphin: graphics settings per game were "among the most requested features", delivered in Release 2503 (March 2025) — [Dolphin progress reports](https://dolphin-emu.org/blog/tags/progress%20report/). Game-specific overrides are reached by right-clicking a game → Properties — [ArchWiki: Dolphin](https://wiki.archlinux.org/title/Dolphin_emulator)
- Dolphin has a "Pause on Focus Lost" option. An old bug linked it to a game-list crash — [Dolphin bug #8739](https://bugs.dolphin-emu.org/issues/8739); [Dolphin forum](https://forums.dolphin-emu.org/Thread-some-suggestions-for-dolphin?pid=460700)

### Inferences
- For gbaa the "game list with per-game settings" collapses: each executable *is* one game, so its settings are per-game by construction. A wizard is unnecessary because there is no BIOS step (HLE/recompiled) and no game directory. Controller setup can be automatic, as in OpenEmu.
- The LaunchBox/DuckStation failure suggests that if gbaa ships a separate launcher, the launcher should only *start* executables and read their metadata. Pause and resume belong inside the game process.

### Gaps
- Whether DuckStation shows a "resume from last state?" prompt at launch (as OpenEmu does), and its "save state on exit" default: not confirmed in sources this pass.
- PCSX2's Big Picture UI and Ryujinx (or forks such as Ryubing) game-list features: not researched within the tool budget.
- ES-DE, Pegasus, Playnite, Steam Big Picture: not researched within the budget. Steam Big Picture is relevant if gbaa executables are added as non-Steam games.

## What "native app feel" means concretely

### Takeaway
The only primary evidence is OpenEmu's stated goal of being a "first class citizen" built on Cocoa, Core Animation and Quartz, and its use of standard ⌘S/⌘L items in a real menu bar ("Controls" menu). The rest is inference.

### Cited Findings
- Its stated goal is to make emulation on OS X "a first class citizen" using Cocoa, Core Animation and Quartz — [GitHub README](https://github.com/clobber/OpenEmu)
- Save State ⌘S and Load State ⌘L appear in the app's **Controls menu** (menu-bar integration with standard shortcuts) — [OpenEmu wiki: Save states](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)
- It uses Metal (2.1) for better performance and battery life, and supports fullscreen — [Wikipedia](https://en.wikipedia.org/wiki/OpenEmu)
- Dialogs that can be suppressed ("don't ask again"), with a reset in Preferences: Library — [OpenEmu wiki: Save states](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)

### Inferences
- Concrete checklist for a per-game gbaa executable on desktop: real OS menu items or standard shortcuts (fullscreen toggle F11/⌘⌃F, quit Ctrl+Q/⌘Q saves before exit); remember window size, position and fullscreen state; HiDPI-correct integer scaling; a proper app icon and window title showing the game name; auto-pause on focus loss (Dolphin has this as an option); dialogs that offer "don't ask again" and can be reset. egui-only menus do not give OS menu-bar integration on macOS, which is a known trade-off.

### Gaps
- No review that defines "native feel" for emulators specifically was found. File associations, window restoration and HiDPI handling in OpenEmu were not verified.

## Error handling: missing BIOS, bad ROM, controller disconnect

### Takeaway
OpenEmu turns errors into specific, plain-language import messages and a resolver UI, not crashes. Controller-disconnect auto-pause is **not** consistently implemented even in Dolphin; there is an open user request for it.

### Cited Findings
- OpenEmu import errors are specific: "No valid system detected", "Already in library", and messages for empty files, missing files referenced by a cue sheet, non-plain-text cue or M3U files, and permissions problems. Ambiguous files go to the **Import Issue Resolver** — [OpenEmu wiki: Importing](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Importing)
- OpenEmu has dedicated wiki pages for BIOS files and "Required m3u file missing", so missing-dependency errors are documented by name — [OpenEmu wiki sidebar](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)
- DuckStation puts the BIOS step first in its setup wizard — [setupwizarddialog.ui](https://git.morozoff.pro/aiden/duckstation/raw/commit/e3c4f77a05070b1e6afd073e8a11726805f1fa76/src/duckstation-qt/setupwizarddialog.ui)
- Dolphin controller disconnect: a 2024 feature request asks for emulation to pause when a controller disconnects, and says the current behaviour is that "the game keeps playing (but sometimes it seems to pause)", which is not consistent — [Dolphin forum feature-request thread](https://forums.dolphin-emu.org/Thread-feature-request-thread?pid=536351)
- OpenEmu silently switches cores when a state needs a different one, so the user never sees that error — [OpenEmu wiki: Save states](https://github.com/OpenEmu/OpenEmu/wiki/User-guide:-Save-states)

### Inferences
- Auto-pause on controller disconnect (as Switch system software does for handheld detachment) is an easy differentiator, since even Dolphin lacks it reliably. For gbaa, missing BIOS and bad ROM largely disappear (the game is compiled in), leaving save-file corruption and controller loss as the main errors to design for.

### Gaps
- Switch and NSO behaviour on controller disconnect, and Delta's: not sourced in this pass. OpenEmu's behaviour on disconnect: not found.
