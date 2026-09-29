//! The Viktor section of `vktr doctor`: is this machine set up to talk to Viktor?
//!
//! It answers the questions a failed first run raises (which key, from where, does it work, which
//! endpoint) with one live `GET /models`, and never prints more of the key than its prefix and
//! last four characters.

use std::time::Instant;

use crate::viktor_login;

/// Print the section. Returns the number of problems found.
pub fn print_report() -> usize {
    let mut problems = 0;
    println!();
    print_header();

    let base_url = match viktor_login::resolve_base_url() {
        Ok(url) => url,
        Err(error) => {
            println!("  ! endpoint                     {error:#}");
            return 1;
        }
    };
    let base_source = if env_set("VIKTOR_BASE_URL") {
        "VIKTOR_BASE_URL"
    } else if base_url.trim_end_matches('/') == "https://api.viktor.com/api/compat/v1" {
        "default"
    } else {
        "config.toml"
    };
    row("·", "endpoint", &format!("{base_url} ({base_source})"));

    let (key, key_source) = if let Some(key) = env_key() {
        (Some(key), "from VIKTOR_API_KEY".to_owned())
    } else if let Some(key) = viktor_login::saved_api_key() {
        let path = xai_grok_config::grok_home().join("config.toml");
        (Some(key), format!("saved in {}", path.display()))
    } else {
        (None, String::new())
    };
    match &key {
        Some(key) => row("·", "api key", &format!("{} ({key_source})", mask(key))),
        None => {
            problems += 1;
            row(
                "!",
                "api key",
                "none: run `vktr login` and paste a key (scope chat:completions), or set VIKTOR_API_KEY",
            );
        }
    }

    if let Some(key) = &key {
        let started = Instant::now();
        let checked = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(viktor_login::verify_api_key(&base_url, key)));
        let ms = started.elapsed().as_millis();
        match checked {
            Ok(models) => row(
                "·",
                "connection",
                &format!("ok in {ms} ms (models: {})", models.join(", ")),
            ),
            Err(error) => {
                problems += 1;
                row("!", "connection", &format!("{error:#}"));
            }
        }
    }

    let backend = std::env::var("VIKTOR_API_BACKEND")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "responses".to_owned());
    let continuation =
        if std::env::var("VKTR_RESPONSES_CONTINUATION").is_ok_and(|v| v.trim() == "0") {
            "off (VKTR_RESPONSES_CONTINUATION=0)"
        } else if backend == "responses" {
            "one Viktor thread per session"
        } else {
            "stateless (only the responses backend continues a thread)"
        };
    row("·", "protocol", &format!("{backend}, {continuation}"));

    let toolset = if xai_grok_shell::agent::config::full_toolset() {
        "full (VKTR_FULL_TOOLSET=1)"
    } else {
        "lean (VKTR_FULL_TOOLSET=1 for workflows, subagents, scheduler, monitor)"
    };
    row("·", "toolset", toolset);

    let sessions = xai_grok_config::grok_home().join("acp").join("sessions");
    let saved = std::fs::read_dir(&sessions)
        .map(|d| {
            d.filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .count()
        })
        .unwrap_or(0);
    let editor_tools = if std::env::var("VKTR_ACP_EDITOR_TOOLS").is_ok_and(|v| v.trim() == "0") {
        "editor tools off"
    } else {
        "editor tools on"
    };
    row(
        "·",
        "acp",
        &format!("{saved} saved editor sessions, {editor_tools}"),
    );

    println!();
    if problems == 0 {
        println!("Viktor: ready");
    } else {
        println!(
            "Viktor: {problems} problem{}",
            if problems == 1 { "" } else { "s" }
        );
    }
    problems
}

/// The official Viktor avatar (white v on the Hero Gradient), box-filtered from `viktor-avatar-512px.svg` to 16 x 16 RGBA pixels.
const AVATAR: &str = include_str!("../assets/avatar16.txt");

/// The section title. On a truecolor terminal it sits beside the avatar tile; piped output, `NO_COLOR` and lesser terminals get the plain `Viktor` line.
fn print_header() {
    use std::io::IsTerminal;
    let truecolor = xai_grok_pager::theme::color_support::detect()
        == xai_grok_pager::theme::color_support::ColorLevel::TrueColor;
    if !std::io::stdout().is_terminal()
        || !truecolor
        || xai_grok_pager::glyphs::is_legacy_windows_console()
    {
        println!("Viktor");
        return;
    }
    let beside = [
        String::new(),
        String::new(),
        "\x1b[1mViktor\x1b[0m".to_owned(),
        format!("\x1b[2mvktr {}\x1b[0m", xai_grok_version::VERSION),
        "\x1b[2mviktor.com\x1b[0m".to_owned(),
    ];
    for (i, tile) in avatar_rows(AVATAR).iter().enumerate() {
        let text = beside.get(i).map_or("", String::as_str);
        println!("  {tile}   {text}");
    }
}

/// One RGBA pixel of the avatar asset.
type Pixel = (u8, u8, u8, u8);

fn parse_avatar(art: &str) -> Vec<Vec<Pixel>> {
    let hex = |s: &str, i: usize| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    art.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            line.split_whitespace()
                .filter_map(|p| Some((hex(p, 0)?, hex(p, 2)?, hex(p, 4)?, hex(p, 6)?)))
                .collect()
        })
        .collect()
}

/// Half-block terminal rows: each cell stacks two pixels (`▀` with the top as fg and the bottom as bg); pixels under half alpha are left to the terminal background.
fn avatar_rows(art: &str) -> Vec<String> {
    let pixels = parse_avatar(art);
    let solid = |p: Option<&Pixel>| p.copied().filter(|p| p.3 >= 128);
    pixels
        .chunks(2)
        .map(|pair| {
            let (top, bottom) = (pair.first(), pair.get(1));
            let width = top.map_or(0, Vec::len);
            let mut out = String::new();
            for x in 0..width {
                let t = solid(top.and_then(|r| r.get(x)));
                let b = solid(bottom.and_then(|r| r.get(x)));
                match (t, b) {
                    (Some((r, g, bl, _)), Some((r2, g2, b2, _))) => out.push_str(&format!(
                        "\x1b[38;2;{r};{g};{bl}m\x1b[48;2;{r2};{g2};{b2}m\u{2580}"
                    )),
                    (Some((r, g, bl, _)), None) => {
                        out.push_str(&format!("\x1b[49m\x1b[38;2;{r};{g};{bl}m\u{2580}"))
                    }
                    (None, Some((r, g, bl, _))) => {
                        out.push_str(&format!("\x1b[49m\x1b[38;2;{r};{g};{bl}m\u{2584}"))
                    }
                    (None, None) => out.push_str("\x1b[0m "),
                }
            }
            out.push_str("\x1b[0m");
            out
        })
        .collect()
}

fn row(mark: &str, name: &str, value: &str) {
    println!("  {mark} {name:<28} {value}");
}

fn env_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.trim().is_empty())
}

fn env_key() -> Option<String> {
    std::env::var("VIKTOR_API_KEY")
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
}

/// `zt_live_sk_…Ab3x`: enough to tell two keys apart, never enough to use one.
fn mask(key: &str) -> String {
    // Viktor's own prefixes are shown whole; anything else only by its first three characters.
    let known = ["zt_live_sk_", "zt_test_sk_"]
        .into_iter()
        .find(|p| key.starts_with(p))
        .map(str::to_owned)
        .unwrap_or_else(|| key.chars().take(3).collect());
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if key.chars().count() <= known.chars().count() + 8 {
        return format!("{known}…");
    }
    format!("{known}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::{AVATAR, avatar_rows, mask, parse_avatar};

    #[test]
    fn the_avatar_asset_is_a_full_16_by_16_tile() {
        let pixels = parse_avatar(AVATAR);
        assert_eq!(pixels.len(), 16);
        assert!(pixels.iter().all(|row| row.len() == 16));
        let rows = avatar_rows(AVATAR);
        assert_eq!(rows.len(), 8, "two pixel rows per terminal row");
        assert!(rows.iter().all(|r| r.matches('\u{2580}').count() == 16));
        // The v is white, the ground is the brand's violet
        assert!(pixels.iter().flatten().any(|p| *p == (255, 255, 255, 255)));
        assert!(pixels.iter().flatten().any(|p| p.2 == 255 && p.0 < 140));
    }

    #[test]
    fn a_masked_key_shows_only_its_kind_and_last_four_characters() {
        let key = "zt_live_sk_0000fake0000fake0000fake0000fake_FAKEFAKEFAKEFAKEWXYZ";
        assert_eq!(mask(key), "zt_live_sk_…WXYZ");
        assert_eq!(mask("zt_test_sk_short"), "zt_test_sk_…");
        assert!(!mask("sk-something-long-enough-1234").contains("something"));
    }
}
