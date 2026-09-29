//! `vktr login`: verify a Viktor public API key against the compat API and save it to
//! `~/.vktr/config.toml` as `[model.viktor] api_key`. The key comes from `--api-key`, from stdin
//! when stdin is not a terminal (`vktr login < key.txt`), or from a prompt with echo off, so it
//! never has to appear on a command line or in shell history.
//!
//! vktr has no browser or device-code login; Viktor issues API keys from its dashboard.

use anyhow::{Context, anyhow};

pub use xai_grok_pager::viktor_key::{
    remove_saved_api_key, save_api_key, saved_api_key, verify_api_key,
};

/// The Viktor API key to use: `VIKTOR_API_KEY` first, then the key saved by `vktr login`.
/// The value is never printed or logged by any caller.
pub fn resolve_api_key() -> Option<String> {
    xai_grok_login::auth_method::read_xai_api_key_env()
        .ok()
        .filter(|k| !k.trim().is_empty())
        .or_else(saved_api_key)
}

/// The Viktor compat API base URL: `[endpoints] viktor_base_url` / `VIKTOR_BASE_URL`, else the
/// public default.
pub fn resolve_base_url() -> anyhow::Result<String> {
    let config = xai_grok_shell::config::load_agent_config_disk_only()
        .map_err(|e| anyhow!("failed to load ~/.vktr/config.toml: {e}"))?;
    Ok(config
        .endpoints
        .resolve_viktor_base_url()
        .trim_end_matches('/')
        .to_owned())
}

/// Read a key without `--api-key`: the first line of stdin when it is piped, otherwise a prompt on
/// the terminal with echo off. `None` when nothing was entered.
pub fn read_api_key_from_user() -> anyhow::Result<Option<String>> {
    use std::io::{BufRead as _, IsTerminal as _, Write as _};
    let stdin = std::io::stdin();
    let mut line = String::new();
    if !stdin.is_terminal() {
        stdin
            .lock()
            .read_line(&mut line)
            .context("read the key from stdin")?;
    } else {
        eprint!("Paste your Viktor API key (zt_live_sk_…; input hidden): ");
        std::io::stderr().flush().ok();
        let _echo = EchoOff::new();
        let read = stdin.lock().read_line(&mut line);
        drop(_echo);
        eprintln!();
        read.context("read the key from the terminal")?;
    }
    let key = line.trim().to_owned();
    Ok((!key.is_empty()).then_some(key))
}

/// Terminal echo switched off for as long as this lives (Unix; a no-op elsewhere).
struct EchoOff {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl EchoOff {
    fn new() -> Self {
        #[cfg(unix)]
        {
            // SAFETY: tcgetattr/tcsetattr on stdin with a zeroed, then filled, termios.
            unsafe {
                let mut term: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut term) != 0 {
                    return Self { saved: None };
                }
                let saved = term;
                term.c_lflag &= !libc::ECHO;
                term.c_lflag |= libc::ECHONL;
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term);
                Self { saved: Some(saved) }
            }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = self.saved {
            // SAFETY: restores the attributes read in `new`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &saved);
            }
        }
    }
}
