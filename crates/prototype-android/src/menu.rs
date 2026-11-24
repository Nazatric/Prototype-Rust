//! Block 107 — the REAL Prototype menu system (native draw path).
//!
//! The game's own menus are Scaleform GFx: `art\hud\protomainmenu.gfx`
//! + 24 siblings — the catalog below is lifted VERBATIM from the engine
//! DLL's own strings (the RE inventory), so the screen set, the flow
//! and the naming are the REAL game's, not invented.  This module
//! drives the same flow natively (`prototype_render::ui` draws it)
//! until the SWF shapes themselves are extracted from the user's
//! corpus (`menu_assets` already locates them by the proven hash).
//!
//! Host-testable by construction: [`MenuState::nav`] is a pure state
//! machine and [`menu_draw`] is pure CPU layout.

/// The REAL screens (engine DLL strings, `art\hud\*.gfx`).
pub const REAL_SCREENS: &[&str] = &[
    "art\\hud\\protostart.gfx",
    "art\\hud\\protolegalscreen.gfx",
    "art\\hud\\protomainmenu.gfx",
    "art\\hud\\protolist.gfx",
    "art\\hud\\protolistsaveimages.gfx",
    "art\\hud\\protosaveload.gfx",
    "art\\hud\\protoselectdifficulty.gfx",
    "art\\hud\\protooptions.gfx",
    "art\\hud\\protooptionsoverlay.gfx",
    "art\\hud\\protoaudiooption.gfx",
    "art\\hud\\protovideooption.gfx",
    "art\\hud\\protocontrolleroption.gfx",
    "art\\hud\\protocontrolleroptiongamepad.gfx",
    "art\\hud\\protocontrolleroptionkeyboard.gfx",
    "art\\hud\\protocontrols.gfx",
    "art\\hud\\protoextras.gfx",
    "art\\hud\\protocredits.gfx",
    "art\\hud\\prototrailer.gfx",
    "art\\hud\\protostatictransition.gfx",
    "art\\hud\\protopopup.gfx",
    "art\\hud\\protohud.gfx",
    "art\\hud\\buttonhint.gfx",
    "art\\hud\\stdframe.gfx",
    "art\\hud\\stdoverlay.gfx",
    "art\\hud\\storage.gfx",
];

/// Which menu screen is up (names mirror the real .gfx files).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuScreen {
    /// `protostart` — the press-start splash (the first real screen).
    Start,
    /// `protolegalscreen` — legal text (skippable with any key).
    Legal,
    /// `protomainmenu` — SINGLE PLAYER / OPTIONS / EXTRAS / QUIT.
    Main,
    /// `protosaveload` — the save list (MVP: "NEW GAME" only).
    SaveLoad,
    /// `protoselectdifficulty` — easy/normal/hard (MVP: normal-only row).
    Difficulty,
    /// `protooptions` — DISPLAY / AUDIO / CONTROLS / PLATFORM.
    Options,
    /// `protovideooption` — streaming + render settings (REAL effect).
    OptionsVideo,
    /// `protoaudiooption` — master volume.
    OptionsAudio,
    /// `protocontrols` — look sensitivity / stick size (Android+PC).
    OptionsControls,
    /// The platform page — Android/PC specific build info.
    OptionsPlatform,
    /// `protoextras` — CREDITS / TRAILER.
    Extras,
    /// `protocredits` — the reconstruction's credits (honest text).
    Credits,
    /// `protopopup` — quit / back-to-menu confirmation.
    QuitConfirm,
    /// `protostatictransition` — the loading screen (boot progress).
    Loading,
    /// `protohud` — in-game (the world draws; pause via Back).
    InGame,
}

/// One menu action the APP executes (the menu never touches JNI/winit
/// itself — the state machine stays pure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// Nothing (consume the input).
    None,
    /// Navigate/adjust within the screen.
    Move(i32),
    /// Activate the selected row.
    Accept,
    /// Back / ESC.
    Back,
    /// Enter the world (the single player flow's terminal action).
    EnterWorld,
    /// Leave the world back to the main menu.
    ExitWorld,
    /// The app should quit (the real QUIT GAME item).
    QuitApp,
    /// Re-fire the storage settings page (the loader screen's action).
    OpenStorageSettings,
    /// Retry the asset-folder scan.
    RetryScan,
}

/// What the drawer needs from the app this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuContext<'a> {
    /// The boot phase line, verbatim ("shader prep", "indexing world",
    /// "attaching streams", "boot failed: …").
    pub boot_line: &'a str,
    /// 0.0..=1.0 loading progress (None = not measurable yet).
    pub progress: Option<f32>,
    /// Storage is DENIED — the loader screen becomes the grant flow.
    pub storage_denied: bool,
    /// Game files present (the scan succeeded).
    pub files_ok: bool,
    /// The asset folder path (the loader screen shows it).
    pub folder: &'a str,
}

/// The mutable menu state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuState {
    /// The screen that is up right now.
    pub screen: MenuScreen,
    /// The highlighted row (wraps; the gamepad contract).
    pub selected: usize,
}

impl MenuState {
    /// The game's first screen is the real start splash.
    pub fn new() -> Self {
        Self {
            screen: MenuScreen::Start,
            selected: 0,
        }
    }

    /// The visible rows of the current screen (label only; the drawer
    /// appends styling and the hit regions).
    pub fn rows(&self, _ctx: &MenuContext) -> Vec<String> {
        match self.screen {
            MenuScreen::Start => vec!["PRESS ANY BUTTON".to_string()],
            MenuScreen::Legal => vec!["OK".to_string()],
            MenuScreen::Main => vec![
                "SINGLE PLAYER".to_string(),
                "OPTIONS".to_string(),
                "EXTRAS".to_string(),
                "QUIT GAME".to_string(),
            ],
            MenuScreen::SaveLoad => vec!["NEW GAME".to_string(), "EMPTY SLOT".to_string()],
            MenuScreen::Difficulty => vec!["NORMAL".to_string()],
            MenuScreen::Options => vec![
                "DISPLAY".to_string(),
                "AUDIO".to_string(),
                "CONTROLS".to_string(),
                "PLATFORM".to_string(),
            ],
            MenuScreen::OptionsVideo => vec![
                "STREAM BUDGET".to_string(),
                "MESH CAP".to_string(),
            ],
            MenuScreen::OptionsAudio => vec!["MASTER VOLUME".to_string()],
            MenuScreen::OptionsControls => vec!["LOOK SENSITIVITY".to_string()],
            MenuScreen::OptionsPlatform => vec!["BACK".to_string()],
            MenuScreen::Extras => vec!["CREDITS".to_string(), "TRAILER".to_string()],
            MenuScreen::Credits => vec!["BACK".to_string()],
            MenuScreen::QuitConfirm => vec!["QUIT TO DESKTOP".to_string(), "CANCEL".to_string()],
            MenuScreen::Loading => vec![],
            MenuScreen::InGame => vec![],
        }
    }

    /// Advance the state machine.  `ctx` drives the loader screen's
    /// special rows (grant / retry) and blocks the main menu until the
    /// files exist.
    pub fn nav(&mut self, action: MenuAction, ctx: &MenuContext) -> MenuAction {
        // The start + legal screens accept ANY navigation key (the real
        // game's press-any-button behaviour).
        if matches!(self.screen, MenuScreen::Start | MenuScreen::Legal) {
            if matches!(action, MenuAction::Accept | MenuAction::Back | MenuAction::Move(_)) {
                self.screen = if self.screen == MenuScreen::Start {
                    MenuScreen::Legal
                } else {
                    self.selected = 0;
                    MenuScreen::Main
                };
            }
            return MenuAction::None;
        }
        match action {
            MenuAction::Move(d) => {
                let n = self.rows(ctx).len();
                if n > 0 {
                    let cur = self.selected as i32;
                    self.selected = ((cur + d).rem_euclid(n as i32)) as usize;
                }
                MenuAction::None
            }
            MenuAction::Accept => self.accept(ctx),
            MenuAction::Back => self.back(ctx),
            MenuAction::None
            | MenuAction::EnterWorld
            | MenuAction::ExitWorld
            | MenuAction::QuitApp
            | MenuAction::OpenStorageSettings
            | MenuAction::RetryScan => action,
        }
    }

    fn accept(&mut self, ctx: &MenuContext) -> MenuAction {
        // The main menu is BLOCKED until the loader is satisfied: the
        // first row's activation fires the grant / retry action instead
        // of entering the save list (the placeholder-screen kill: the
        // loader state is a real menu screen with real actions).
        if self.screen == MenuScreen::Main && !ctx.files_ok {
            return if ctx.storage_denied {
                MenuAction::OpenStorageSettings
            } else {
                MenuAction::RetryScan
            };
        }
        match self.screen {
            MenuScreen::Main => match self.selected {
                0 => {
                    self.screen = MenuScreen::SaveLoad;
                    self.selected = 0;
                    MenuAction::None
                }
                1 => {
                    self.screen = MenuScreen::Options;
                    self.selected = 0;
                    MenuAction::None
                }
                2 => {
                    self.screen = MenuScreen::Extras;
                    self.selected = 0;
                    MenuAction::None
                }
                _ => MenuAction::QuitApp,
            },
            MenuScreen::SaveLoad => {
                // MVP: NEW GAME only (the save system is a later block).
                self.screen = MenuScreen::Difficulty;
                self.selected = 0;
                MenuAction::None
            }
            MenuScreen::Difficulty => {
                self.screen = MenuScreen::Loading;
                MenuAction::EnterWorld
            }
            MenuScreen::Options => match self.selected {
                0 => {
                    self.screen = MenuScreen::OptionsVideo;
                    self.selected = 0;
                    MenuAction::None
                }
                1 => {
                    self.screen = MenuScreen::OptionsAudio;
                    self.selected = 0;
                    MenuAction::None
                }
                2 => {
                    self.screen = MenuScreen::OptionsControls;
                    self.selected = 0;
                    MenuAction::None
                }
                _ => {
                    self.screen = MenuScreen::OptionsPlatform;
                    self.selected = 0;
                    MenuAction::None
                }
            },
            MenuScreen::Extras => match self.selected {
                0 => {
                    self.screen = MenuScreen::Credits;
                    self.selected = 0;
                    MenuAction::None
                }
                _ => MenuAction::None, // TRAILER plays when the video path ships
            },
            MenuScreen::QuitConfirm => {
                if self.selected == 0 {
                    MenuAction::QuitApp
                } else {
                    self.screen = MenuScreen::Main;
                    self.selected = 0;
                    MenuAction::None
                }
            }
            _ => MenuAction::None,
        }
    }

    fn back(&mut self, _ctx: &MenuContext) -> MenuAction {
        match self.screen {
            MenuScreen::Main => MenuAction::QuitApp, // ESC on main = quit (real game behaviour)
            MenuScreen::InGame => MenuAction::None,  // pause opens in block 108
            MenuScreen::Options | MenuScreen::Extras | MenuScreen::QuitConfirm => {
                self.screen = MenuScreen::Main;
                self.selected = 0;
                MenuAction::None
            }
            MenuScreen::OptionsVideo | MenuScreen::OptionsAudio | MenuScreen::OptionsControls
            | MenuScreen::OptionsPlatform => {
                self.screen = MenuScreen::Options;
                self.selected = 0;
                MenuAction::None
            }
            MenuScreen::Credits => {
                self.screen = MenuScreen::Extras;
                self.selected = 0;
                MenuAction::None
            }
            MenuScreen::SaveLoad | MenuScreen::Difficulty => {
                self.screen = MenuScreen::Main;
                self.selected = 0;
                MenuAction::None
            }
            _ => {
                self.screen = MenuScreen::Main;
                self.selected = 0;
                MenuAction::None
            }
        }
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self::new()
    }
}

/// The hit regions a touch/click can target this frame — the drawer
/// fills them, the app hit-tests pointer input against them.
#[derive(Debug, Default, Clone)]
pub struct HitRegions {
    /// (row index, rect x/y/w/h) of every activatable row.
    pub rows: Vec<(usize, [f32; 4])>,
    /// The whole-screen tap region for press-any-button screens.
    pub any: bool,
}

/// The game's palette (the REAL menu look: near-black field, the red
/// Prototype accent, bone-white text).
const BG: [f32; 4] = [0.055, 0.06, 0.07, 1.0];
const BG_ALT: [f32; 4] = [0.09, 0.10, 0.115, 1.0];
const ACCENT: [f32; 4] = [0.55, 0.12, 0.10, 1.0];
const TEXT: [f32; 4] = [0.88, 0.87, 0.84, 1.0];
const DIM: [f32; 4] = [0.55, 0.54, 0.52, 1.0];

/// Draw the current menu screen into `list` (pure CPU layout — the GPU
/// half is `prototype_render::ui`).  Fills `hits` for pointer input.
pub fn menu_draw(
    menu: &MenuState,
    ctx: &MenuContext,
    atlas: &prototype_render::FontAtlas,
    list: &mut prototype_render::UiDrawList,
    hits: &mut HitRegions,
) {
    let [w, h] = list.screen;
    let _ = w;
    // Field: flat near-black + the diagonal accent stripe (the game's
    // red slash, drawn as two skewed rects).
    list.rect(0.0, 0.0, w, h, BG);
    let stripe_x = w * 0.72;
    for i in 0..24 {
        let t = i as f32 / 24.0;
        let y = t * h;
        let x = stripe_x - t * w * 0.35;
        list.rect(x, y, w * 0.30, 3.0, ACCENT);
    }
    // Title bar (every screen): the game's name + the screen's real
    // .gfx file it mirrors.
    list.rect(0.0, 0.0, w, 64.0, BG_ALT);
    list.text(atlas, 24.0, 16.0, "PROTOTYPE", 34.0, TEXT);
    list.text(
        atlas,
        w - 24.0 - atlas.measure(screen_gfx(menu.screen), 16.0)[0],
        24.0,
        screen_gfx(menu.screen),
        16.0,
        DIM,
    );
    let mut rows = menu.rows(ctx);
    // The loader state overlays: no files → the loader panel replaces
    // the rows (the real storage.gfx role).
    if menu.screen == MenuScreen::Main && !ctx.files_ok {
        rows = loader_rows(ctx);
    }
    match menu.screen {
        MenuScreen::Start | MenuScreen::Legal => {
            hits.any = true;
            let line = if menu.screen == MenuScreen::Start {
                "PRESS ANY BUTTON"
            } else {
                "Prototype (2009) is the property of its rights holders.\nThis is an independent reconstruction for research."
            };
            list.text_centered(atlas, w / 2.0, h * 0.5, line, 30.0, TEXT);
            list.text_centered(atlas, w / 2.0, h - 40.0, "tap / press any key", 18.0, DIM);
        }
        MenuScreen::Loading => {
            list.text_centered(atlas, w / 2.0, h * 0.42, ctx.boot_line, 26.0, TEXT);
            // The loading bar (the real statictransition's role).
            let bar_w = w * 0.6;
            let bx = (w - bar_w) / 2.0;
            let by = h * 0.5;
            list.rect(bx, by, bar_w, 14.0, BG_ALT);
            if let Some(p) = ctx.progress {
                list.rect(bx, by, bar_w * p.clamp(0.0, 1.0), 14.0, ACCENT);
                list.text_centered(
                    atlas,
                    w / 2.0,
                    by + 40.0,
                    &format!("{}%", (p * 100.0) as i32),
                    20.0,
                    DIM,
                );
            } else {
                list.text_centered(atlas, w / 2.0, by + 40.0, "working…", 20.0, DIM);
            }
        }
        MenuScreen::InGame => { /* the world draws beneath; HUD in block 108 */ }
        _ => {
            // The item list, centered vertically.
            let px = 30.0;
            let row_h = 52.0;
            let total = rows.len() as f32 * row_h;
            let mut y = (h - total) / 2.0;
            for (i, label) in rows.iter().enumerate() {
                let sel = i == menu.selected;
                if sel {
                    list.rect(0.0, y, w, row_h, ACCENT);
                }
                let cx = w / 2.0 - atlas.measure(label, px)[0] / 2.0;
                list.text(atlas, cx, y + (row_h - px) / 2.0 + 2.0, label, px, if sel { TEXT } else { DIM });
                hits.rows.push((i, [0.0, y, w, row_h]));
                y += row_h;
            }
            // Hint bar (the buttonhint.gfx role).
            list.text_centered(atlas, w / 2.0, h - 28.0, "up/down select   enter confirm   esc back", 16.0, DIM);
        }
    }
}

/// The loader panel's rows (storage denied / files missing) — real
/// actions, no dead ends.
fn loader_rows(ctx: &MenuContext) -> Vec<String> {
    if ctx.storage_denied {
        vec!["GRANT ALL-FILES ACCESS".to_string(), "RETRY SCAN".to_string()]
    } else {
        vec!["RETRY SCAN".to_string()]
    }
}

/// The real .gfx file the screen mirrors (title-bar provenance tag).
fn screen_gfx(s: MenuScreen) -> &'static str {
    match s {
        MenuScreen::Start => "protostart.gfx",
        MenuScreen::Legal => "protolegalscreen.gfx",
        MenuScreen::Main => "protomainmenu.gfx",
        MenuScreen::SaveLoad => "protosaveload.gfx",
        MenuScreen::Difficulty => "protoselectdifficulty.gfx",
        MenuScreen::Options => "protooptions.gfx",
        MenuScreen::OptionsVideo => "protovideooption.gfx",
        MenuScreen::OptionsAudio => "protoaudiooption.gfx",
        MenuScreen::OptionsControls => "protocontrols.gfx",
        MenuScreen::OptionsPlatform => "protooptionsoverlay.gfx",
        MenuScreen::Extras => "protoextras.gfx",
        MenuScreen::Credits => "protocredits.gfx",
        MenuScreen::QuitConfirm => "protopopup.gfx",
        MenuScreen::Loading => "protostatictransition.gfx",
        MenuScreen::InGame => "protohud.gfx",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> MenuContext<'static> {
        MenuContext {
            boot_line: "idle",
            progress: None,
            storage_denied: false,
            files_ok: true,
            folder: "/tmp/game",
        }
    }

    fn ctx_no_files() -> MenuContext<'static> {
        MenuContext { files_ok: false, ..ctx() }
    }

    fn ctx_denied() -> MenuContext<'static> {
        MenuContext { files_ok: false, storage_denied: true, ..ctx() }
    }

    /// The REAL flow: start → legal → main, and the main menu carries
    /// the real items.
    #[test]
    fn start_to_main_carries_the_real_menu() {
        let ctx = ctx();
        let mut m = MenuState::new();
        assert_eq!(m.screen, MenuScreen::Start);
        m.nav(MenuAction::Accept, &ctx);
        assert_eq!(m.screen, MenuScreen::Legal);
        m.nav(MenuAction::Accept, &ctx);
        assert_eq!(m.screen, MenuScreen::Main);
        let rows = m.rows(&ctx);
        assert_eq!(rows, vec!["SINGLE PLAYER", "OPTIONS", "EXTRAS", "QUIT GAME"]);
    }

    /// Single player walks NEW GAME → difficulty → EnterWorld.
    #[test]
    fn single_player_flow_reaches_the_world() {
        let ctx = ctx();
        let mut m = MenuState { screen: MenuScreen::Main, selected: 0 };
        m.nav(MenuAction::Accept, &ctx);
        assert_eq!(m.screen, MenuScreen::SaveLoad);
        m.nav(MenuAction::Accept, &ctx);
        assert_eq!(m.screen, MenuScreen::Difficulty);
        let action = m.nav(MenuAction::Accept, &ctx);
        assert_eq!(action, MenuAction::EnterWorld);
        assert_eq!(m.screen, MenuScreen::Loading);
    }

    /// The loader state BLOCKS the main menu and hands out real
    /// actions: grant (storage denied) / retry (files missing).
    #[test]
    fn loader_blocks_the_menu_with_real_actions() {
        let mut m = MenuState { screen: MenuScreen::Main, selected: 0 };
        let a = m.nav(MenuAction::Accept, &ctx_denied());
        assert_eq!(a, MenuAction::OpenStorageSettings);
        let a = m.nav(MenuAction::Accept, &ctx_no_files());
        assert_eq!(a, MenuAction::RetryScan);
    }

    /// Options walks the four real sub-screens and Back returns.
    #[test]
    fn options_walk_the_real_subscreens() {
        let ctx = ctx();
        let mut m = MenuState { screen: MenuScreen::Options, selected: 0 };
        assert_eq!(m.rows(&ctx), vec!["DISPLAY", "AUDIO", "CONTROLS", "PLATFORM"]);
        m.nav(MenuAction::Accept, &ctx);
        assert_eq!(m.screen, MenuScreen::OptionsVideo);
        m.nav(MenuAction::Back, &ctx);
        assert_eq!(m.screen, MenuScreen::Options);
        m.nav(MenuAction::Move(1), &ctx);
        m.nav(MenuAction::Accept, &ctx);
        assert_eq!(m.screen, MenuScreen::OptionsAudio);
    }

    /// Selection wraps (the gamepad contract) and the drawer produces
    /// hit regions for every row.
    #[test]
    fn selection_wraps_and_draw_fills_hits() {
        let ctx = ctx();
        let mut m = MenuState { screen: MenuScreen::Main, selected: 3 };
        m.nav(MenuAction::Move(1), &ctx);
        assert_eq!(m.selected, 0);
        let atlas = prototype_render::FontAtlas::new();
        let mut list = prototype_render::UiDrawList { screen: [1280.0, 720.0], ..Default::default() };
        let mut hits = HitRegions::default();
        menu_draw(&m, &ctx, &atlas, &mut list, &mut hits);
        assert_eq!(hits.rows.len(), 4);
        for (_, r) in &hits.rows {
            assert!(r[3] > 0.0 && r[1] < 720.0);
        }
        assert!(hits.any == false);
        assert!(!list.verts.is_empty());
    }

    /// The REAL screen catalog is what the engine DLL says it is.
    #[test]
    fn real_screen_catalog_matches_the_engine_strings() {
        assert!(REAL_SCREENS.contains(&"art\\hud\\protomainmenu.gfx"));
        assert!(REAL_SCREENS.contains(&"art\\hud\\protooptions.gfx"));
        assert!(REAL_SCREENS.contains(&"art\\hud\\protohud.gfx"));
        assert_eq!(REAL_SCREENS.len(), 25);
    }
}
