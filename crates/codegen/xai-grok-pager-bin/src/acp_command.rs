//! `vktr acp`: serve Viktor to an editor over the Agent Client Protocol on stdio.
//!
//! This replaces the standalone TypeScript `viktor-acp` package. The protocol runs on stdout, so
//! this command is dispatched before any other start-up work and writes nothing else there;
//! diagnostics go to stderr.
//!
//! `vktr agent stdio` is a different thing: it serves vktr's *local* coding agent over ACP. This
//! command serves Viktor working in its own cloud sandbox; it reaches the local workspace only
//! through the file and terminal capabilities the editor offers, with the editor asking the user
//! before any write or command. `VKTR_ACP_EDITOR_TOOLS=0` turns that off.

use std::rc::Rc;

use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

/// Run the agent until the editor disconnects. Returns the process exit code.
pub(crate) fn run(args: &xai_grok_pager::app::cli::AcpArgs) -> i32 {
    if let Some(editor) = &args.print_config {
        print_editor_config(editor);
        return 0;
    }
    xai_grok_extra_ca::ensure_default_crypto_provider();

    let base_url = match crate::viktor_login::resolve_base_url() {
        Ok(base_url) => base_url,
        Err(error) => {
            eprintln!("vktr acp: {error:#}");
            return 1;
        }
    };
    // The handshake is allowed to succeed without a key so the editor can show the auth method
    // and its fix hint, exactly as the TypeScript agent did; prompts fail until a key is set.
    let api_key = crate::viktor_login::resolve_api_key().unwrap_or_else(|| {
        eprintln!(
            "vktr acp: no Viktor API key. The handshake will work; prompts fail until you set \
             VIKTOR_API_KEY or run `vktr login`."
        );
        String::new()
    });

    let client = match vktr_acp::ViktorClient::new(api_key, &base_url) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("vktr acp: {error}");
            return 1;
        }
    };
    eprintln!("vktr acp: serving Viktor at {}", client.compat_base_url());
    let config = vktr_acp::AgentConfig {
        // Sessions survive an agent restart: `session/load` finds the same Viktor thread.
        store: Some(vktr_acp::SessionStore::in_home(
            &xai_grok_config::grok_home(),
        )),
        editor_tools: std::env::var("VKTR_ACP_EDITOR_TOOLS").map_or(true, |v| v.trim() != "0"),
        // The editor's MCP servers become Viktor tools; VKTR_ACP_MCP=0 leaves them unconnected.
        mcp_servers: std::env::var("VKTR_ACP_MCP").map_or(true, |v| v.trim() != "0"),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("vktr acp: could not start the async runtime: {error}");
            return 1;
        }
    };
    let local = tokio::task::LocalSet::new();
    let result = local.block_on(&runtime, async move {
        vktr_acp::serve(
            Rc::new(client),
            config,
            tokio::io::stdout().compat_write(),
            tokio::io::stdin().compat(),
        )
        .await
    });
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("vktr acp: {}", error.message);
            1
        }
    }
}

/// The snippet an editor needs to run `vktr acp`. Uses the `vktr` on PATH when it is this binary
/// (a stable link such as ~/.local/bin/vktr survives updates), else this binary's own path.
fn print_editor_config(editor: &str) {
    let command = binary_path();
    let (file, snippet) = match editor {
        "zed" => (
            "~/.config/zed/settings.json (merge into \"agent_servers\")",
            serde_json::json!({
                "agent_servers": {
                    "Viktor": {"type": "custom", "command": command, "args": ["acp"]}
                }
            }),
        ),
        _ => (
            "~/.jetbrains/acp.json",
            serde_json::json!({
                "agent_servers": {"Viktor": {"command": command, "args": ["acp"]}}
            }),
        ),
    };
    eprintln!("Add this to {file}. The key comes from `vktr login` or VIKTOR_API_KEY.");
    println!(
        "{}",
        serde_json::to_string_pretty(&snippet).unwrap_or_default()
    );
}

fn binary_path() -> String {
    let me = std::env::current_exe().ok();
    let canonical = me.as_ref().and_then(|p| std::fs::canonicalize(p).ok());
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("vktr");
            if canonical.is_some() && std::fs::canonicalize(&candidate).ok() == canonical {
                return candidate.display().to_string();
            }
        }
    }
    me.map_or_else(|| "vktr".to_owned(), |p| p.display().to_string())
}
