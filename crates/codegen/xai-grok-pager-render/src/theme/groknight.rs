//! GrokNight theme (shown as vktr Night): the Viktor brand palette on a violet-tinted near-black.
//!
//! Brand solids from viktor.com/brand: Peach #FFBD9E, Lilac #947FFF, Violet #6748FD, Navy #150079, Soft-black #1B182A.
//! The grounds follow the brand's soft-black hue pushed darker, the way the Viktor decks do; semantic colors (error, success, diff) stay conventional.
//!
//! The canonical palette is defined in RGB (`Color::Rgb`).
//! At startup [`Theme::quantized`] downgrades every color to the terminal's detected capability level (256-color, 16-color, etc.).

use ratatui::style::{Color, Modifier};

use super::tokyonight::Theme;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

#[allow(dead_code)]
mod palette {
    use super::*;

    // ── Backgrounds (brand soft-black hue, darkened) ────────────────────
    pub const BG: Color = rgb(11, 10, 18); //  #0b0a12, Night (terminal bg)
    pub const BG_DARK: Color = rgb(13, 11, 21); //  #0d0b15, darkest
    pub const BG_STORM_DARK: Color = rgb(17, 15, 26); //  #110f1a, dark bg
    pub const BG_STORM: Color = rgb(21, 19, 31); //  #15131f, main bg
    pub const BG_HIGHLIGHT: Color = rgb(38, 34, 54); //  #262236, highlight bg

    // ── Text / grays (a lilac cast, after the deck's #a9a3cc muted) ─────
    pub const FG: Color = rgb(232, 229, 242); // #e8e5f2, primary text
    pub const FG_DARK: Color = rgb(203, 199, 219); // #cbc7db, secondary text
    pub const FG_GUTTER: Color = rgb(66, 61, 85); //  #423d55, dim
    pub const COMMENT: Color = rgb(116, 109, 140); //  #746d8c, muted
    pub const DARK3: Color = rgb(95, 89, 119); //  #5f5977, medium gray
    pub const DARK5: Color = rgb(133, 127, 160); // #857fa0, bright gray

    // ── Viktor brand ────────────────────────────────────────────────────
    pub const PEACH: Color = rgb(255, 189, 158); // #FFBD9E
    pub const LILAC: Color = rgb(148, 127, 255); // #947FFF
    pub const VIOLET: Color = rgb(103, 72, 253); // #6748FD
    pub const NAVY: Color = rgb(21, 0, 121); // #150079
    pub const LILAC_MIST: Color = rgb(179, 163, 255); // #B3A3FF, the blur-and-blend gradient's lilac
    pub const PEACH_MIST: Color = rgb(255, 204, 181); // #FFCCB5, the blur-and-blend gradient's peach
    pub const DECK_LILAC: Color = rgb(165, 141, 255); // #A58DFF, wordmark cells in the Viktor decks
    pub const DECK_LILAC_DEEP: Color = rgb(132, 110, 220); // #846EDC, their second tone
    // Hero Gradient stops: linear-gradient(135deg, #1E0079, #7749FF 45%, #9580FF 70%, #FFBD9E 88%, #FFF5AC)
    pub const HERO_NAVY: Color = rgb(30, 0, 121); // #1E0079, purple-500
    pub const HERO_VIOLET: Color = rgb(119, 73, 255); // #7749FF, purple-400 (primary)
    pub const HERO_LILAC: Color = rgb(149, 128, 255); // #9580FF, purple-300
    pub const HERO_YELLOW: Color = rgb(255, 245, 172); // #FFF5AC, accent yellow

    // ── Semantic accents ────────────────────────────────────────────────
    pub const BLUE: Color = rgb(138, 164, 255); // #8aa4ff, leans toward the lilac
    pub const CYAN: Color = rgb(125, 207, 255); // #7dcfff
    pub const GREEN: Color = rgb(158, 206, 106); // #9ece6a
    pub const RED: Color = rgb(247, 118, 142); // #f7768e
    pub const YELLOW: Color = rgb(230, 190, 120); // #e6be78
    pub const TEAL: Color = rgb(26, 188, 156); // #1abc9c

    pub const RED_DARK: Color = rgb(66, 14, 20); // #420e14, quantizes to 256-color red, not gray
    pub const GREEN_DARK: Color = rgb(6, 56, 6); // #063806, quantizes to 256-color green, not gray
}
use palette::*;

impl Theme {
    pub const fn groknight() -> Self {
        Self {
            bg_base: BG_STORM,
            bg_light: BG_HIGHLIGHT,
            bg_dark: rgb(29, 26, 41), // #1d1a29, lighter than bg_base for visible code blocks
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: rgb(46, 42, 64),
            bg_terminal: BG,

            accent_user: FG_DARK,
            accent_assistant: LILAC,
            accent_thinking: DARK5,
            accent_tool: DARK5,
            accent_system: BLUE,
            accent_error: RED,
            accent_success: GREEN,
            accent_running: LILAC,
            accent_skill: LILAC_MIST,

            text_primary: FG,
            text_secondary: FG_DARK,

            gray_dim: rgb(90, 84, 112), // #5a5470, slightly brighter than FG_GUTTER
            gray: COMMENT,
            gray_bright: DARK5,

            command: YELLOW,
            path: PEACH,
            running: CYAN,
            warning: YELLOW,

            fuzzy_accent: PEACH,

            accent_plan: PEACH,

            accent_verify: LILAC_MIST,

            accent_remember: Color::Rgb(139, 195, 74), // #8BC34A, Material Design light green

            selection_border: rgb(64, 57, 90),
            prompt_border: rgb(52, 46, 74), // #342e4a, dimmer prompt chrome
            prompt_border_active: rgb(104, 88, 176), // #6858b0, the lilac lights up when focused
            hover_border: rgb(33, 30, 46),

            accent_model: PEACH,

            scrollbar_bg: BG_STORM_DARK,
            scrollbar_fg: BG_HIGHLIGHT,

            diff_delete_bg: RED_DARK,
            diff_delete_fg: RED,
            diff_insert_bg: GREEN_DARK,
            diff_insert_fg: GREEN,
            diff_equal_fg: COMMENT,
            diff_gutter_fg: COMMENT,

            bg_visual: rgb(57, 51, 80),

            paste_bg: BG_STORM_DARK,
            paste_fg: FG_DARK,
            paste_dim: FG_GUTTER,

            md_heading_h1: PEACH,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: LILAC,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: LILAC_MIST,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: DARK5, // bright gray
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: COMMENT, // medium gray
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: DARK3, // medium gray, unbold
            md_heading_h6_mod: Modifier::empty(),
            md_code: PEACH_MIST,
            md_task_checked: GREEN,
            md_task_unchecked: FG_DARK, // text_secondary
            md_muted: COMMENT,
            md_code_bg: rgb(29, 26, 41),
            md_text: FG_DARK,
            link_fg: BLUE,

            brand: DECK_LILAC,
            brand_deep: DECK_LILAC_DEEP,
            brand_glint: rgb(255, 255, 255),
            // The Hero Gradient, stop for stop (Viktor brand guidelines)
            brand_gradient: [HERO_NAVY, HERO_VIOLET, HERO_LILAC, PEACH, HERO_YELLOW],
        }
    }
}
