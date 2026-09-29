//! Home splash: the wordmark centered on the screen with one status line beneath it, and the composer at the bottom.
//!
//! It replaces the framed hero box and the stacked menu once the user is signed in with access.
//! Only notices that need the user (startup warnings, announcements, the update / privacy / resume slot) are added around it.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::render::line_utils::truncate_line;
use crate::theme::Theme;

use super::logo::LogoTier;
use super::{PROMPT_HEIGHT, VERSION_GAP, WelcomeLayout, WelcomeLayoutInput};

/// Rows between the wordmark and the status line.
const STATUS_GAP: u16 = 1;

/// Rows the wordmark block reserves below the art: the gap, the status line, and the optional notice blocks with their gaps.
fn rows_below_logo(error_height: u16, info_height: u16) -> u16 {
    let error = if error_height > 0 {
        1 + error_height
    } else {
        0
    };
    let info = if info_height > 0 { 1 + info_height } else { 0 };
    STATUS_GAP + 1 + error + info
}

/// Lay out the splash. `input.menu_height` is ignored: the splash draws no menu.
pub(super) fn compute_splash(input: &WelcomeLayoutInput<'_>) -> WelcomeLayout {
    let content_area = input.content_area;
    let height = content_area.height;
    let error_height = input.error_height;
    let tip_height = input.tip_height;
    let prompt_height = input.prompt_height.unwrap_or(PROMPT_HEIGHT);
    let one_line_prompt = prompt_height.min(PROMPT_HEIGHT);
    let fixed_below = WelcomeLayout::fixed_below(tip_height, prompt_height);
    let flex_gap = 1u16;

    // The announcement outranks the wordmark: its rows are measured against a one-line prompt with the art hidden, then the tier steps down to make room
    let info_budget = height.saturating_sub(
        rows_below_logo(error_height, 0)
            + 1
            + flex_gap
            + WelcomeLayout::fixed_below(tip_height, one_line_prompt),
    );
    let info_height = if input.announcement.is_some() {
        super::stacked_announcement_rows(input).min(info_budget)
    } else {
        0
    };
    let below_logo = rows_below_logo(error_height, info_height);

    // No menu competes for rows, so start from the full mark (hidden on a legacy console) and shrink only when it does not fit
    let mut tier = LogoTier::for_hero(true);
    while tier.width() > content_area.width
        || tier.rows() + below_logo + flex_gap + fixed_below > height
    {
        match tier.step_down() {
            Some(smaller) => tier = smaller,
            None => break,
        }
    }
    let block = tier.rows() + below_logo;

    // Center the block on the whole content area; a taller draft eats the space below it before moving it up
    let centered = height.saturating_sub(block) / 2;
    let top_pad = centered.min(height.saturating_sub(block + flex_gap + fixed_below));

    let error_gap = if error_height > 0 { 1 } else { 0 };
    let info_gap = if info_height > 0 { 1 } else { 0 };
    let tip_gap = if tip_height > 0 { 1 } else { 0 };
    let [
        _,
        logo,
        _,
        status,
        _,
        error,
        _,
        info,
        _,
        tip,
        _,
        prompt,
        _,
        version,
    ] = Layout::vertical([
        Constraint::Length(top_pad),
        Constraint::Length(tier.rows()),
        Constraint::Length(STATUS_GAP),
        Constraint::Length(1),
        Constraint::Length(error_gap),
        Constraint::Length(error_height),
        Constraint::Length(info_gap),
        Constraint::Length(info_height),
        Constraint::Min(flex_gap),
        Constraint::Length(tip_height),
        Constraint::Length(tip_gap),
        Constraint::Length(prompt_height),
        Constraint::Length(VERSION_GAP),
        Constraint::Length(1),
    ])
    .areas(content_area);

    let zero = Rect::default();
    WelcomeLayout {
        logo,
        error,
        menu: zero,
        changelog: info,
        tip,
        prompt,
        version,
        status,
        hero_box: zero,
        hero_logo: zero,
        hero_version: zero,
        hero_subtitle: zero,
        hero_info: zero,
        hero_menu: zero,
        logo_tier: tier,
    }
}

/// Paint the status line centered in `area`: `vktr VERSION · branch cwd`, then the team and API-key sign-in when they apply.
/// A line wider than the area is cut from the right, so the version always shows.
pub(super) fn render_status_line(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    team_name: Option<&str>,
    is_api_key_auth: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let muted = Style::default().fg(theme.gray);
    let sep = Span::styled("  ·  ", Style::default().fg(theme.gray_dim));

    let mut spans = vec![
        Span::styled(
            "vktr",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " {}{}",
                xai_grok_version::VERSION,
                xai_grok_update::channel_label()
            ),
            muted,
        ),
    ];
    spans.push(sep.clone());
    spans.extend(super::top_bar::location_line(theme).spans);
    if let Some(team) = team_name {
        spans.push(sep.clone());
        spans.push(Span::styled(team.to_string(), muted));
    }
    if is_api_key_auth {
        spans.push(sep);
        spans.push(Span::styled("API key", muted));
    }

    let line = truncate_line(Line::from(spans), area.width as usize);
    let width = (line.width() as u16).min(area.width);
    let x = area.x + (area.width - width) / 2;
    buf.set_line(x, area.y, &line, width);
}
