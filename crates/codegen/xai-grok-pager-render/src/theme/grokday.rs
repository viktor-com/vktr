//! GrokDay (shown as vktr Day) is the light counterpart to vktr Night.
//! Grounds are a faint lilac white and text is the brand soft-black (#1B182A).
//! Accents are the Viktor brand hues deepened for contrast on light backgrounds: Violet and Navy carry the brand, peach deepens to terracotta.

use ratatui::style::{Color, Modifier};

use super::tokyonight::Theme;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

#[allow(dead_code)]
mod palette {
    use super::*;

    // ── Backgrounds (lilac-tinted light) ─────────────────────────────────
    pub const BG: Color = rgb(248, 247, 252); // #f8f7fc — brightest (terminal bg)
    pub const BG_DARK: Color = rgb(243, 241, 250); // #f3f1fa
    pub const BG_STORM_DARK: Color = rgb(235, 232, 246); // #ebe8f6
    pub const BG_STORM: Color = rgb(241, 239, 248); // #f1eff8 — main bg
    pub const BG_HIGHLIGHT: Color = rgb(225, 221, 240); // #e1ddf0 — highlight bg
    pub const SCROLLBAR_THUMB: Color = rgb(221, 217, 237); // #ddd9ed — ≥30 Σ darker than the track

    // ── Text / grays (soft-black ramp) ───────────────────────────────────
    pub const FG: Color = rgb(27, 24, 42); // #1B182A — brand soft-black, primary text
    pub const FG_DARK: Color = rgb(60, 55, 80); // #3c3750 — secondary text
    pub const FG_GUTTER: Color = rgb(182, 177, 200); // #b6b1c8 — dim
    pub const COMMENT: Color = rgb(116, 110, 138); // #746e8a — muted
    pub const DARK3: Color = rgb(143, 138, 162); // #8f8aa2 — medium gray
    pub const DARK5: Color = rgb(96, 90, 118); // #605a76 — bright gray

    // ── Viktor brand ─────────────────────────────────────────────────────
    pub const VIOLET: Color = rgb(103, 72, 253); // #6748FD
    pub const LILAC: Color = rgb(148, 127, 255); // #947FFF
    pub const NAVY: Color = rgb(21, 0, 121); // #150079
    pub const INDIGO: Color = rgb(64, 40, 190); // #4028BE, between Violet and Navy
    pub const VIOLET_DEEP: Color = rgb(82, 54, 226); // #5236E2, the wordmark's second tone, one step darker than Violet
    pub const TERRACOTTA: Color = rgb(196, 88, 44); // #C4582C, Peach deepened for light grounds
    pub const HERO_NAVY: Color = rgb(30, 0, 121); // #1E0079, the Hero Gradient's first stop
    pub const HERO_VIOLET: Color = rgb(119, 73, 255); // #7749FF, primary
    pub const AMBER: Color = rgb(190, 140, 20); // #BE8C14, the Hero Gradient's yellow deepened for light grounds

    // ── Semantic accents ─────────────────────────────────────────────────
    pub const BLUE: Color = rgb(47, 90, 214); // #2F5AD6
    pub const CYAN: Color = rgb(0, 130, 170); // #0082AA
    pub const GREEN: Color = rgb(55, 142, 35); // #378E23
    pub const RED: Color = rgb(205, 48, 72); // #CD3048
    pub const YELLOW: Color = rgb(162, 118, 18); // #A27612

    pub const RED_LIGHT: Color = rgb(245, 218, 222); // #F5DADE — diff delete bg
    pub const GREEN_LIGHT: Color = rgb(218, 242, 220); // #DAF2DC — diff insert bg
}
use palette::*;

impl Theme {
    pub const fn grokday() -> Self {
        Self {
            bg_base: BG_STORM,
            bg_light: BG_HIGHLIGHT,
            bg_dark: rgb(230, 227, 243),
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: rgb(214, 209, 234),
            bg_terminal: BG,

            accent_user: FG_DARK,
            accent_assistant: VIOLET,
            accent_thinking: DARK5,
            accent_tool: DARK5,
            accent_system: BLUE,
            accent_error: RED,
            accent_success: GREEN,
            accent_running: VIOLET,
            accent_skill: INDIGO,

            text_primary: FG,
            text_secondary: FG_DARK,

            gray_dim: rgb(166, 160, 186), // #a6a0ba — slightly darker than FG_GUTTER
            gray: COMMENT,
            gray_bright: DARK5,

            command: YELLOW,
            path: TERRACOTTA,
            running: CYAN,
            warning: YELLOW,

            fuzzy_accent: VIOLET,

            accent_plan: TERRACOTTA,

            accent_verify: INDIGO,

            accent_remember: rgb(76, 175, 80), // #4CAF50 — Material Design green (readable on light bg)

            selection_border: rgb(186, 180, 210),
            prompt_border: rgb(203, 198, 222), // #cbc6de — dimmer prompt chrome
            prompt_border_active: rgb(150, 132, 235), // #9684eb — lilac when focused
            hover_border: rgb(214, 210, 228),

            accent_model: TERRACOTTA,

            scrollbar_bg: BG_STORM_DARK,
            scrollbar_fg: SCROLLBAR_THUMB,

            diff_delete_bg: RED_LIGHT,
            diff_delete_fg: RED,
            diff_insert_bg: GREEN_LIGHT,
            diff_insert_fg: GREEN,
            diff_equal_fg: COMMENT,
            diff_gutter_fg: COMMENT,

            bg_visual: rgb(202, 195, 232),

            paste_bg: BG_HIGHLIGHT,
            paste_fg: FG_DARK,
            paste_dim: FG_GUTTER,

            md_heading_h1: TERRACOTTA,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: VIOLET,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: NAVY,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: DARK5,
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: COMMENT,
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: DARK3,
            md_heading_h6_mod: Modifier::empty(),
            md_code: INDIGO,
            md_task_checked: GREEN,
            md_task_unchecked: FG_DARK,
            md_muted: COMMENT,
            md_code_bg: rgb(230, 227, 243),
            md_text: FG_DARK,
            link_fg: BLUE,

            brand: VIOLET,
            brand_deep: VIOLET_DEEP,
            brand_glint: rgb(255, 255, 255),
            // The Hero Gradient deepened for light grounds: peach and yellow would wash out on #f1eff8
            brand_gradient: [HERO_NAVY, HERO_VIOLET, LILAC, TERRACOTTA, AMBER],
        }
    }
}
