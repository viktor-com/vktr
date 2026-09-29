//! The Viktor API key: verify it against the compat API and keep it in `~/.vktr/config.toml` as
//! `[model.viktor] api_key` (owner-only). Shared by `vktr login` / `vktr logout` and the TUI's
//! sign-in screen, so both save the key the same way.

use std::path::PathBuf;

use anyhow::{Context as _, anyhow};

/// `GET {base_url}/models` with the key. Returns the advertised model ids on success and the
/// server's own error message (with the HTTP status) otherwise, so a missing `chat:completions`
/// scope or a bad key is shown verbatim.
pub async fn verify_api_key(base_url: &str, key: &str) -> anyhow::Result<Vec<String>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client =
        xai_grok_extra_ca::build_reqwest_client(|b| b.timeout(std::time::Duration::from_secs(20)))
            .map_err(|e| anyhow!("build HTTP client: {e}"))?;
    let resp = client
        .get(&url)
        .bearer_auth(key)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // OpenAI-style `{"error": {"message"}}` and Viktor's REST `{"detail": {"message"}}` or
        // `{"detail": "..."}`.
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                ["/error/message", "/detail/message", "/detail"]
                    .iter()
                    .find_map(|p| v.pointer(p).and_then(|m| m.as_str()).map(str::to_owned))
            })
            .unwrap_or_else(|| body.chars().take(300).collect());
        return Err(anyhow!("HTTP {status}: {detail}"));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).context("models response is not JSON")?;
    Ok(v.get("data")
        .and_then(|d| d.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn config_path() -> PathBuf {
    xai_grok_config::grok_home().join("config.toml")
}

/// `[model.viktor] api_key` as saved by `vktr login`.
pub fn saved_api_key() -> Option<String> {
    let doc: toml_edit::DocumentMut = std::fs::read_to_string(config_path()).ok()?.parse().ok()?;
    doc.get("model")?
        .get("viktor")?
        .get("api_key")?
        .as_str()
        .map(str::to_owned)
        .filter(|k| !k.trim().is_empty())
}

/// Write `[model.viktor] api_key = "<key>"` into the user config, preserving everything else.
/// The file is owner-only on Unix.
pub fn save_api_key(key: &str) -> anyhow::Result<PathBuf> {
    let path = config_path();
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = existing
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
    set_viktor_key(&mut doc, key).with_context(|| path.display().to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    write_private(&path, doc.to_string().as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn set_viktor_key(doc: &mut toml_edit::DocumentMut, key: &str) -> anyhow::Result<()> {
    let model = doc
        .entry("model")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let model_tbl = model
        .as_table_mut()
        .ok_or_else(|| anyhow!("[model] is not a table"))?;
    model_tbl.set_implicit(true);
    let viktor = model_tbl
        .entry("viktor")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let viktor_tbl = viktor
        .as_table_mut()
        .ok_or_else(|| anyhow!("[model.viktor] is not a table"))?;
    viktor_tbl["api_key"] = toml_edit::value(key);
    Ok(())
}

/// Remove `[model.viktor] api_key`. Returns the file it was removed from, or `None` when no key
/// was saved.
pub fn remove_saved_api_key() -> anyhow::Result<Option<PathBuf>> {
    let path = config_path();
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let mut doc: toml_edit::DocumentMut = existing
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
    let removed = doc
        .get_mut("model")
        .and_then(|m| m.get_mut("viktor"))
        .and_then(|v| v.as_table_like_mut())
        .and_then(|t| t.remove("api_key"))
        .is_some();
    if !removed {
        return Ok(None);
    }
    write_private(&path, doc.to_string().as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(Some(path))
}

/// Create or truncate `path` owner-only, so a key never sits in a world-readable file, even
/// briefly.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_the_key_keeps_the_rest_of_the_config() {
        let mut doc: toml_edit::DocumentMut =
            "[ui]\ntheme = \"dark\"\n\n[model.local]\nmodel = \"x\"\n"
                .parse()
                .expect("toml");
        set_viktor_key(&mut doc, "zt_live_sk_test").expect("set");
        let text = doc.to_string();
        assert!(text.contains("theme = \"dark\""));
        assert!(text.contains("[model.local]"));
        assert!(text.contains("[model.viktor]"));
        assert!(text.contains("api_key = \"zt_live_sk_test\""));
    }
}
