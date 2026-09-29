//! The Viktor wordmark, drawn from square pixel cells the way the Viktor decks draw it.
//!
//! The art is the wordmark's vector outline rasterized onto a pixel grid (`#` is a lit cell).
//! Each terminal cell carries two pixels, stacked with the half-block glyphs `▀ ▄ █`.
//! Terminal cells run taller than two square pixels, so the art is rasterized 1.2x shorter than the outline to keep the letters' proportions.
//! Lit pixels dither between two brand tones along a slowly drifting ramp (Bayer 4x4), and a white glint sweeps the mark twice.
//! Then the mark holds still, so an idle welcome screen stops redrawing and the event loop can park.
//!
//! The mark is hidden on legacy Windows consoles, whose raster fonts and 16-color palette would render it as a flat block.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::sync::OnceLock;

use crate::theme::Theme;

/// 57 x 14 pixels (7 rows): the deck's 57-cell grid.
const LOGO: &str = include_str!("../../../assets/logo/wordmark57.txt");
/// 46 x 11 pixels (6 rows), for narrower or shorter windows and the minimal welcome card.
const LOGO_SMALL: &str = include_str!("../../../assets/logo/wordmark46.txt");

/// A wordmark parsed once: `lit[y * w + x]` for pixel `(x, y)`.
#[derive(Debug)]
struct Art {
    w: usize,
    h: usize,
    lit: Vec<bool>,
}

impl Art {
    fn parse(logo: &str) -> Self {
        let rows: Vec<&str> = logo.lines().filter(|l| !l.is_empty()).collect();
        let w = rows.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let mut lit = vec![false; w * rows.len()];
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                if let Some(p) = lit.get_mut(y * w + x) {
                    *p = c == '#';
                }
            }
        }
        Self {
            w,
            h: rows.len(),
            lit,
        }
    }

    fn full() -> &'static Self {
        static ART: OnceLock<Art> = OnceLock::new();
        ART.get_or_init(|| Self::parse(LOGO))
    }

    fn small() -> &'static Self {
        static ART: OnceLock<Art> = OnceLock::new();
        ART.get_or_init(|| Self::parse(LOGO_SMALL))
    }

    fn lit(&self, x: usize, y: usize) -> bool {
        x < self.w && self.lit.get(y * self.w + x).copied().unwrap_or(false)
    }

    /// Terminal rows: two pixel rows per cell, rounded up.
    fn rows(&self) -> u16 {
        self.h.div_ceil(2) as u16
    }

    fn cols(&self) -> u16 {
        self.w as u16
    }
}

/// Height at or above which the small logo is shown (below it, no logo).
const SMALL_LOGO_MIN_HEIGHT: u16 = 21;
/// Height at or above which the full logo is shown.
const FULL_LOGO_MIN_HEIGHT: u16 = 26;

/// Which logo art the stacked column shows.
/// The terminal height picks the tier; the stacked layout steps it down only while the column would not fit beside the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoTier {
    Full,
    Compact,
    Hidden,
}

impl LogoTier {
    pub fn for_height(window_height: u16) -> Self {
        Self::for_height_and_hidden(window_height, logo_hidden())
    }

    /// Takes the legacy-console flag as a parameter so tests can drive it directly.
    fn for_height_and_hidden(window_height: u16, hidden: bool) -> Self {
        if hidden || window_height < SMALL_LOGO_MIN_HEIGHT {
            Self::Hidden
        } else if window_height < FULL_LOGO_MIN_HEIGHT {
            Self::Compact
        } else {
            Self::Full
        }
    }

    fn art(self) -> Option<&'static Art> {
        match self {
            Self::Full => Some(Art::full()),
            Self::Compact => Some(Art::small()),
            Self::Hidden => None,
        }
    }

    pub fn rows(self) -> u16 {
        self.art().map_or(0, Art::rows)
    }

    pub fn width(self) -> u16 {
        self.art().map_or(0, Art::cols)
    }

    /// The hero box's mark: `Full` when the box is wide enough to keep a readable column beside it, else `Compact`; hidden on a legacy console.
    pub fn for_hero(roomy: bool) -> Self {
        if logo_hidden() {
            Self::Hidden
        } else if roomy {
            Self::Full
        } else {
            Self::Compact
        }
    }

    /// The next smaller tier; `None` once hidden.
    pub fn step_down(self) -> Option<Self> {
        match self {
            Self::Full => Some(Self::Compact),
            Self::Compact => Some(Self::Hidden),
            Self::Hidden => None,
        }
    }
}

fn pick_logo(window_height: u16) -> Option<&'static Art> {
    pick_logo_for(window_height, logo_hidden())
}

fn pick_logo_for(window_height: u16, hidden: bool) -> Option<&'static Art> {
    LogoTier::for_height_and_hidden(window_height, hidden).art()
}

/// See the module doc.
fn logo_hidden() -> bool {
    crate::glyphs::is_legacy_windows_console()
}

/// Seconds since the welcome screen first asked about the mark.
/// The phase is wall-clock based so the drift and glint speeds are independent of the frame rate.
fn elapsed_secs() -> f32 {
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// When the mark stops moving: just after the second glint sweep ends (~11 s).
const SETTLE_SECS: f32 = GLINT_DELAY + GLINT_CYCLE + GLINT_SECS + 0.1;

/// Animation phase for `elapsed` seconds: it stops at [`SETTLE_SECS`].
fn phase_at(elapsed: f32) -> f32 {
    elapsed.min(SETTLE_SECS)
}

fn anim_phase_secs() -> f32 {
    phase_at(elapsed_secs())
}

/// Whether the mark still moves; once false, the welcome screen no longer needs ticks for it.
pub fn logo_animating() -> bool {
    !logo_hidden() && elapsed_secs() < SETTLE_SECS
}

/// When the hero box's gradient edge starts drawing in, and how long it takes to cross the box.
/// It runs inside the wordmark's first glint so the whole welcome animation still settles at [`SETTLE_SECS`].
const EDGE_REVEAL_DELAY: f32 = 0.15;
const EDGE_REVEAL_SECS: f32 = 0.9;

/// How far the hero box's gradient edge has drawn in, `0..=1` (eased), or `None` on a legacy console where the edge stays a plain border.
pub fn edge_reveal_progress() -> Option<f32> {
    edge_reveal_progress_for(anim_phase_secs(), logo_hidden())
}

fn edge_reveal_progress_for(secs: f32, hidden: bool) -> Option<f32> {
    if hidden {
        return None;
    }
    let t = ((secs - EDGE_REVEAL_DELAY) / EDGE_REVEAL_SECS).clamp(0.0, 1.0);
    // Ease out: quick start, soft landing
    Some(1.0 - (1.0 - t).powi(3))
}

/// Redraw cadence in frames per second.
/// The drift is slow and the glint moves a pixel column at a time, so a few fps reads as pixels stepping, as in the deck, while sparing the long-lived welcome screen from full-rate repaints.
const SHIMMER_FPS: f32 = 12.0;

/// Quantized animation frame for the current wall-clock phase.
/// The welcome screen redraws only when this advances, throttling the animation to ~`SHIMMER_FPS` rather than the full event-loop tick rate.
/// The frame is pinned to 0 when the logo is hidden, and stops advancing once the mark settles.
pub fn shimmer_frame() -> u64 {
    if logo_hidden() {
        return 0;
    }
    (anim_phase_secs() * SHIMMER_FPS) as u64
}

const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// Ordered-dither threshold in `(0, 1)` for pixel `(x, y)`.
fn bayer_threshold(x: usize, y: usize) -> f32 {
    let level = BAYER.get((y & 3) * 4 + (x & 3)).copied().unwrap_or(0);
    (level as f32 + 0.5) / 16.0
}

/// How a lit pixel is painted this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Brand,
    Deep,
    Glint,
}

/// Seconds between the two glint sweeps; the first one starts shortly after the screen opens.
const GLINT_CYCLE: f32 = 9.0;
const GLINT_DELAY: f32 = 0.4;
const GLINT_SECS: f32 = 1.5;
/// Half-widths of the glint's core and its brand-colored shoulders, in normalized sweep units.
const GLINT_CORE: f32 = 0.045;
const GLINT_SHOULDER: f32 = 0.11;
/// Radians per second the two-tone ramp drifts.
const DRIFT: f32 = 0.45;

/// Tone of the lit pixel at `(x, y)` in a `w` x `h` pixel mark at time `secs`.
/// The two tones follow a diagonal sine ramp thresholded through the Bayer matrix, and the ramp's phase drifts over time.
/// A band sweeps left to right, leaning with the rows; its core takes the glint and its shoulders the full brand tone.
fn tone_at(x: usize, y: usize, w: usize, h: usize, secs: f32) -> Tone {
    let fx = x as f32 / w.max(1) as f32;
    let fy = y as f32 / h.max(1) as f32;

    let t = secs - GLINT_DELAY;
    if t >= 0.0 {
        let sp = (t % GLINT_CYCLE) / GLINT_SECS;
        if sp <= 1.0 {
            let band = -0.35 + sp * 1.7;
            let d = ((fx + fy * 0.3) / 1.3 - band).abs();
            if d < GLINT_CORE {
                return Tone::Glint;
            }
            if d < GLINT_SHOULDER {
                return Tone::Brand;
            }
        }
    }

    let u = 0.5 + 0.5 * (fx * 2.4 + fy * 1.2 - secs * DRIFT).sin();
    if u > bayer_threshold(x, y) {
        Tone::Brand
    } else {
        Tone::Deep
    }
}

fn tone_color(theme: &Theme, tone: Tone) -> Color {
    match tone {
        Tone::Brand => theme.brand,
        Tone::Deep => theme.brand_deep,
        Tone::Glint => theme.brand_glint,
    }
}

/// Paint `art` horizontally centered at the top of `area`, clipped to it.
fn render_into(area: Rect, buf: &mut Buffer, theme: &Theme, art: &Art, secs: f32) {
    let (w, h) = (art.w, art.h);
    let lit = |x: usize, y: usize| art.lit(x, y);
    let color = |x: usize, y: usize| tone_color(theme, tone_at(x, y, w, h, secs));

    let x0 = area.x + area.width.saturating_sub(art.cols()) / 2;
    let cols = art.cols().min(area.width);
    let rows = art.rows().min(area.height);
    for row in 0..rows {
        for col in 0..cols {
            let (x, top_y, bottom_y) = (col as usize, row as usize * 2, row as usize * 2 + 1);
            let (top, bottom) = (lit(x, top_y), lit(x, bottom_y));
            let cell = &mut buf[(x0 + col, area.y + row)];
            match (top, bottom) {
                (false, false) => continue,
                (true, false) => {
                    cell.set_symbol("▀").set_fg(color(x, top_y));
                }
                (false, true) => {
                    cell.set_symbol("▄").set_fg(color(x, bottom_y));
                }
                (true, true) => {
                    let (fg, bg) = (color(x, top_y), color(x, bottom_y));
                    if fg == bg {
                        cell.set_symbol("█").set_fg(fg);
                    } else {
                        cell.set_symbol("▀").set_fg(fg).set_bg(bg);
                    }
                }
            }
        }
    }
}

/// Paint the largest art no bigger than `preferred` that fits `area`, so a narrow window shrinks the mark instead of clipping it.
fn render_fitting(area: Rect, buf: &mut Buffer, theme: &Theme, preferred: LogoTier) {
    let mut tier = preferred;
    loop {
        let Some(art) = tier.art() else { return };
        if art.cols() <= area.width && art.rows() <= area.height {
            render_into(area, buf, theme, art, anim_phase_secs());
            return;
        }
        match tier.step_down() {
            Some(next) => tier = next,
            None => return,
        }
    }
}

pub fn logo_line_count(window_height: u16) -> u16 {
    pick_logo(window_height).map_or(0, Art::rows)
}

pub fn logo_visual_width(window_height: u16) -> u16 {
    pick_logo(window_height).map_or(24, Art::cols)
}

pub fn render_logo(area: Rect, buf: &mut Buffer, theme: &Theme, window_height: u16) {
    render_logo_tier(area, buf, theme, LogoTier::for_height(window_height));
}

/// Paint the tier the layout reserved rows for, so the art can never outgrow its slot.
pub fn render_logo_tier(area: Rect, buf: &mut Buffer, theme: &Theme, tier: LogoTier) {
    if !logo_hidden() {
        render_fitting(area, buf, theme, tier);
    }
}

/// The full logo's size, independent of the height-based [`pick_logo`] tiers used by the stacked layout.
/// When [`logo_hidden`], they report 0 and render nothing.
pub fn full_logo_line_count() -> u16 {
    full_logo_line_count_for(logo_hidden())
}

fn full_logo_line_count_for(hidden: bool) -> u16 {
    if hidden { 0 } else { Art::full().rows() }
}

pub fn full_logo_visual_width() -> u16 {
    full_logo_visual_width_for(logo_hidden())
}

fn full_logo_visual_width_for(hidden: bool) -> u16 {
    if hidden { 0 } else { Art::full().cols() }
}

/// The hero box paints into a rect sized for its tier, so preferring the full mark lands on whichever tier the layout picked.
pub fn render_full_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    render_logo_tier(area, buf, theme, LogoTier::Full);
}

/// Line count of the small logo used in minimal's committed welcome card (0 on a legacy Windows console, where the mark is suppressed).
pub fn compact_logo_line_count() -> u16 {
    if logo_hidden() {
        0
    } else {
        Art::small().rows()
    }
}

/// Render the small wordmark (centered) into `area` for minimal's welcome card.
/// No-op when the logo is hidden.
pub fn render_compact_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    render_logo_tier(area, buf, theme, LogoTier::Compact);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gradient_edge_draws_in_then_holds_before_the_mark_settles() {
        assert_eq!(edge_reveal_progress_for(0.0, false), Some(0.0));
        let mid = edge_reveal_progress_for(EDGE_REVEAL_DELAY + EDGE_REVEAL_SECS / 2.0, false);
        assert!(mid.is_some_and(|p| p > 0.5 && p < 1.0), "eased out: {mid:?}");
        assert_eq!(edge_reveal_progress_for(EDGE_REVEAL_DELAY + EDGE_REVEAL_SECS, false), Some(1.0));
        assert_eq!(edge_reveal_progress_for(SETTLE_SECS, false), Some(1.0));
        const { assert!(EDGE_REVEAL_DELAY + EDGE_REVEAL_SECS < SETTLE_SECS) };
        assert_eq!(edge_reveal_progress_for(1.0, true), None, "plain border on a legacy console");
    }

    #[test]
    fn logo_sizes_by_height() {
        let tier = |h| LogoTier::for_height_and_hidden(h, false);
        assert_eq!(tier(SMALL_LOGO_MIN_HEIGHT - 1), LogoTier::Hidden);
        assert_eq!(tier(SMALL_LOGO_MIN_HEIGHT), LogoTier::Compact);
        assert_eq!(tier(FULL_LOGO_MIN_HEIGHT - 1), LogoTier::Compact);
        assert_eq!(tier(FULL_LOGO_MIN_HEIGHT), LogoTier::Full);
    }

    #[test]
    fn logo_hidden_on_legacy_console_at_every_height() {
        for h in [0, SMALL_LOGO_MIN_HEIGHT, FULL_LOGO_MIN_HEIGHT, u16::MAX] {
            assert!(pick_logo_for(h, true).is_none(), "height {h}");
        }
    }

    #[test]
    fn full_logo_is_the_large_variant() {
        assert_eq!(full_logo_line_count_for(false), Art::full().rows());
        assert_eq!(full_logo_visual_width_for(false), Art::full().cols());
        assert!(full_logo_line_count_for(false) > Art::small().rows());
        assert!(full_logo_visual_width_for(false) > Art::small().cols());
    }

    #[test]
    fn full_logo_helpers_collapse_when_hidden() {
        assert_eq!(full_logo_line_count_for(true), 0);
        assert_eq!(full_logo_visual_width_for(true), 0);
    }

    #[test]
    fn compact_logo_line_count_matches_small_logo_when_visible() {
        // The minimal welcome card budgets exactly the small logo's rows
        if !logo_hidden() {
            assert_eq!(compact_logo_line_count(), Art::small().rows());
            assert!(compact_logo_line_count() < Art::full().rows());
            assert!(compact_logo_line_count() > 0);
        } else {
            assert_eq!(compact_logo_line_count(), 0);
        }
    }

    #[test]
    fn art_is_rectangular_pixel_grids() {
        for logo in [LOGO, LOGO_SMALL] {
            let rows: Vec<&str> = logo.lines().filter(|l| !l.is_empty()).collect();
            assert!(
                rows.iter()
                    .all(|l| l.chars().count() == rows[0].chars().count()),
                "ragged rows"
            );
            assert!(
                rows.iter().all(|l| l.chars().all(|c| c == '#' || c == '.')),
                "only # and . are pixels"
            );
        }
        assert_eq!((Art::full().w, Art::full().h), (57, 14));
        assert_eq!((Art::small().w, Art::small().h), (46, 11));
        assert_eq!(Art::full().rows(), 7);
        assert_eq!(Art::small().rows(), 6);
    }

    /// The outline is 216 x 64, so square pixels would give 57 x 16.9; the art is 1.2x shorter than that.
    #[test]
    fn art_is_squashed_to_the_cell_aspect() {
        for art in [Art::full(), Art::small()] {
            let square_h = art.w as f32 * 64.0 / 216.0;
            let squash = square_h / art.h as f32;
            assert!(
                (1.1..1.3).contains(&squash),
                "squash {squash} for {}x{}",
                art.w,
                art.h
            );
        }
    }

    fn painted(theme: &Theme, art: &Art, secs: f32) -> Buffer {
        let area = Rect::new(0, 0, art.cols(), art.rows());
        let mut buf = Buffer::empty(area);
        render_into(area, &mut buf, theme, art, secs);
        buf
    }

    #[test]
    fn every_lit_pixel_is_painted_and_nothing_else() {
        let theme = Theme::groknight();
        let art = Art::full();
        let buf = painted(&theme, art, 3.0);
        for row in 0..art.rows() {
            for col in 0..art.cols() {
                let (x, y) = (col as usize, row as usize * 2);
                let symbol = buf[(col, row)].symbol();
                match (art.lit(x, y), art.lit(x, y + 1)) {
                    (false, false) => assert_eq!(symbol, " ", "cell {col},{row}"),
                    (true, false) => assert_eq!(symbol, "▀", "cell {col},{row}"),
                    (false, true) => assert_eq!(symbol, "▄", "cell {col},{row}"),
                    (true, true) => assert!(matches!(symbol, "█" | "▀"), "cell {col},{row}"),
                }
            }
        }
    }

    #[test]
    fn wordmark_dithers_between_both_brand_tones() {
        let theme = Theme::groknight();
        let buf = painted(&theme, Art::full(), 5.0); // between sweeps
        let fgs: Vec<Color> = buf
            .content()
            .iter()
            .filter_map(|c| (c.symbol() != " ").then_some(c.fg))
            .collect();
        assert!(fgs.contains(&theme.brand));
        assert!(fgs.contains(&theme.brand_deep));
        assert!(!fgs.contains(&theme.brand_glint), "no glint at rest");
    }

    #[test]
    fn the_glint_is_white_on_the_default_themes() {
        assert_eq!(Theme::groknight().brand_glint, Color::Rgb(255, 255, 255));
        assert_eq!(Theme::grokday().brand_glint, Color::Rgb(255, 255, 255));
    }

    fn glint_cols(secs: f32) -> Vec<usize> {
        let (w, h) = (57, 14);
        (0..w)
            .filter(|&x| (0..h).any(|y| tone_at(x, y, w, h, secs) == Tone::Glint))
            .collect()
    }

    #[test]
    fn glint_sweeps_left_to_right_twice() {
        let early = glint_cols(GLINT_DELAY + 0.5);
        let late = glint_cols(GLINT_DELAY + 1.0);
        assert!(!early.is_empty() && !late.is_empty());
        assert!(
            early.first() < late.first(),
            "the band advances: {early:?} then {late:?}"
        );
        assert!(!glint_cols(GLINT_DELAY + GLINT_CYCLE + 0.5).is_empty());
        assert!(glint_cols(GLINT_DELAY + 5.0).is_empty());
    }

    /// Once settled the phase is pinned, so the painted frame and the redraw counter stop changing.
    #[test]
    fn the_mark_settles_after_the_second_sweep() {
        const { assert!(SETTLE_SECS > GLINT_DELAY + GLINT_CYCLE + GLINT_SECS) };
        const { assert!(SETTLE_SECS < 12.0) };
        assert!(
            glint_cols(SETTLE_SECS).is_empty(),
            "settles without a glint"
        );
        let theme = Theme::groknight();
        let settled = painted(&theme, Art::full(), phase_at(SETTLE_SECS));
        assert_eq!(
            settled,
            painted(&theme, Art::full(), phase_at(SETTLE_SECS + 30.0))
        );
        assert_ne!(
            settled,
            painted(&theme, Art::full(), phase_at(SETTLE_SECS - 3.0))
        );
        let frame = |elapsed: f32| (phase_at(elapsed) * SHIMMER_FPS) as u64;
        assert_eq!(frame(SETTLE_SECS + 1.0), frame(SETTLE_SECS + 600.0));
    }

    #[test]
    fn narrow_area_falls_back_to_the_small_mark() {
        let theme = Theme::groknight();
        let area = Rect::new(0, 0, 50, 7);
        let mut buf = Buffer::empty(area);
        render_fitting(area, &mut buf, &theme, LogoTier::Full);
        let painted_cols: Vec<u16> = (0..50)
            .filter(|&c| (0..7).any(|r| buf[(c, r)].symbol() != " "))
            .collect();
        assert!(!painted_cols.is_empty());
        assert!(painted_cols.last().unwrap() - painted_cols[0] < 46);
    }
}
