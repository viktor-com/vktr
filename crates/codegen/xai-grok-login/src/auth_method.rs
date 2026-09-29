//! First-party API-key environment primitives.
//!
//! Only the env-key checks the auth subsystem itself needs live here. The ACP
//! `auth_methods` list-building surface (`build_auth_methods`,
//! `AuthMethodsBuildInputs`, `should_advertise_xai_api_key`, ...) stays in
//! `xai_grok_shell::agent::auth_method`, which depends on shell's `ModelEntry`.

/// Env var that, when set, advertises `xai.api_key` as a viable auth method.
///
/// Kept as a constant so test code and the production check stay in sync.
pub const XAI_API_KEY_ENV_VAR: &str = "VIKTOR_API_KEY";

/// Legacy env var name.
/// Checked as a fallback when `XAI_API_KEY` is not set, so existing deployments that use the old name keep working.
pub const LEGACY_XAI_API_KEY_ENV_VAR: &str = "XAI_API_KEY";

/// Read the API key from the environment.
///
/// Checks `XAI_API_KEY` first, then falls back to the legacy `VKTR_CODE_XAI_API_KEY` for backward compatibility.
pub fn read_xai_api_key_env() -> Result<String, std::env::VarError> {
    std::env::var(XAI_API_KEY_ENV_VAR)
        .or_else(|_| std::env::var(LEGACY_XAI_API_KEY_ENV_VAR))
        .or_else(|e| signed_in_api_key().ok_or(e))
}

/// A key the user signed in with while this process runs (vktr's TUI sign-in screen): it counts
/// as this process's API key from then on, after the environment. Kept here rather than put into
/// the environment, which a running multi-threaded process must not change.
static SIGNED_IN_API_KEY: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

pub fn set_signed_in_api_key(key: Option<String>) {
    let key = key.map(|k| k.trim().to_owned()).filter(|k| !k.is_empty());
    *SIGNED_IN_API_KEY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = key;
}

fn signed_in_api_key() -> Option<String> {
    SIGNED_IN_API_KEY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Returns `true` if either `XAI_API_KEY` or `VKTR_CODE_XAI_API_KEY` is set.
pub fn has_xai_api_key_env() -> bool {
    read_xai_api_key_env().is_ok()
}
