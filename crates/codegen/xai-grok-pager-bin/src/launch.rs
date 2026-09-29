//! `vktr launch`: run a host coding tool against a local OpenAI-compatible backend, or against the
//! Viktor compat API (`--viktor`), without hand-editing that tool's provider config.
//!
//! Modelled on `harbor launch` (github.com/av/harbor). When Harbor is installed, backend detection
//! and start-up are delegated to it rather than reimplemented. Without Harbor, vktr probes the
//! well-known local ports of the common backends, discovers models from `/v1/models`, and wires the
//! selected model into the tool through the same environment variables and config files Harbor uses.
//!
//! Everything after the tool name belongs to the tool. The tool runs from the invoking directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use xai_grok_pager::app::cli::LaunchArgs;

/// Host tools vktr can wire up itself.
const NATIVE_TOOLS: &[&str] = &[
    "claude", "codex", "copilot", "grok", "hermes", "mi", "opencode", "pi", "pool", "vktr",
];
/// Tools that can be pointed at the Viktor compat API (it speaks chat completions, responses and
/// Anthropic messages, so every OpenAI-compatible tool plus `claude` qualifies).
const VIKTOR_TOOLS: &[&str] = &[
    "claude", "codex", "copilot", "grok", "hermes", "mi", "opencode", "pi", "pool", "vktr",
];
/// Tools only Harbor knows how to configure.
const HARBOR_ONLY_TOOLS: &[&str] = &["droid", "openclaw", "vscode"];

/// Well-known local endpoints, probed in this order when `--backend` is not given.
const BACKENDS: &[(&str, &[&str])] = &[
    ("ollama", &["http://127.0.0.1:11434"]),
    (
        "llamacpp",
        &["http://127.0.0.1:8080", "http://127.0.0.1:33831"],
    ),
    ("lmstudio", &["http://127.0.0.1:1234"]),
    ("vllm", &["http://127.0.0.1:8000", "http://127.0.0.1:33911"]),
    ("sglang", &["http://127.0.0.1:30000"]),
    ("tabbyapi", &["http://127.0.0.1:5000"]),
    ("mistralrs", &["http://127.0.0.1:8021"]),
    ("dmr", &["http://127.0.0.1:12434/engines"]),
    ("mlx", &["http://127.0.0.1:10240"]),
    ("litellm", &["http://127.0.0.1:4000"]),
];

const LOCAL_API_KEY: &str = "sk-local";

/// Where the tool should point, fully resolved.
#[derive(Debug, Clone)]
struct Target {
    /// Short label used in provider names (`ollama`, `viktor`, `custom`).
    backend: String,
    /// Root URL without `/v1` (what `ANTHROPIC_BASE_URL`-style settings want).
    base_url: String,
    /// OpenAI-style API root, ending in `/v1`.
    api_url: String,
    api_key: String,
    model: String,
    models: Vec<String>,
    viktor: bool,
}

/// What to do for one tool: environment, leading arguments, and config files to merge.
#[derive(Debug, Default)]
struct Plan {
    program: String,
    env: BTreeMap<String, String>,
    unset_env: Vec<String>,
    lead_args: Vec<String>,
    /// (path, description) of files written before launch.
    files: Vec<(PathBuf, &'static str)>,
    notes: Vec<String>,
}

pub async fn run(args: &LaunchArgs) -> anyhow::Result<()> {
    let (tool, tool_args) = args.tool_and_args.split_first().ok_or_else(|| {
        anyhow!(
            "usage: vktr launch [--backend <svc>] [--model <m>] [--config] [--viktor] <tool> [args]"
        )
    })?;
    let tool = tool.as_str();

    if args.viktor {
        if !VIKTOR_TOOLS.contains(&tool) {
            bail!(
                "`{tool}` cannot be pointed at the Viktor compat API by vktr. Supported with --viktor: {}",
                VIKTOR_TOOLS.join(", ")
            );
        }
        if args.backend.is_some() {
            bail!("--viktor and --backend are mutually exclusive");
        }
    } else if tool != "vktr" && harbor_available() {
        // Harbor owns backend detection, start-up and the full tool list: hand over unchanged.
        return delegate_to_harbor(args, tool, tool_args);
    } else if HARBOR_ONLY_TOOLS.contains(&tool) {
        bail!(
            "`{tool}` is configured by Harbor only. Install Harbor (https://github.com/av/harbor) and re-run."
        );
    } else if !NATIVE_TOOLS.contains(&tool) {
        bail!(
            "unknown tool `{tool}`. Supported: {}{}",
            NATIVE_TOOLS.join(", "),
            if harbor_available() {
                ""
            } else {
                " (install Harbor for droid, openclaw, vscode)"
            }
        );
    }

    let target = if args.viktor {
        viktor_target(args.model.as_deref())?
    } else {
        local_target(args.backend.as_deref(), args.model.as_deref()).await?
    };
    let plan = plan_for(tool, &target, tool_args)?;

    if args.config {
        write_files(tool, &target, &plan)?;
        print_config(tool, &target, &plan, tool_args);
        return Ok(());
    }
    if find_on_path(&plan.program).is_none() {
        bail!("`{}` is not installed or not on PATH", plan.program);
    }
    write_files(tool, &target, &plan)?;
    for note in &plan.notes {
        eprintln!("vktr launch: {note}");
    }
    exec(&plan, tool_args)
}

// ---------------------------------------------------------------------------------------------
// Harbor delegation
// ---------------------------------------------------------------------------------------------

fn harbor_available() -> bool {
    std::env::var_os("VKTR_LAUNCH_NO_HARBOR").is_none() && find_on_path("harbor").is_some()
}

fn delegate_to_harbor(args: &LaunchArgs, tool: &str, tool_args: &[String]) -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new("harbor");
    cmd.arg("launch");
    if let Some(backend) = &args.backend {
        cmd.args(["--backend", backend]);
    }
    if let Some(model) = &args.model {
        cmd.args(["--model", model]);
    }
    if args.config {
        cmd.arg("--config");
    }
    cmd.arg(tool).args(tool_args);
    exec_command(cmd, "harbor")
}

// ---------------------------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------------------------

fn viktor_target(model: Option<&str>) -> anyhow::Result<Target> {
    let config = xai_grok_shell::config::load_agent_config_disk_only()
        .map_err(|e| anyhow!("failed to load ~/.vktr/config.toml: {e}"))?;
    let api_url = config.endpoints.resolve_viktor_base_url();
    let api_url = api_url.trim_end_matches('/').to_owned();
    let base_url = api_url.strip_suffix("/v1").unwrap_or(&api_url).to_owned();
    let api_key = crate::viktor_login::resolve_api_key().ok_or_else(|| {
        anyhow!(
            "no Viktor API key: export VIKTOR_API_KEY or run `vktr login` \
                 (a Viktor public API key with the chat:completions scope)"
        )
    })?;
    let model = model.unwrap_or("viktor").to_owned();
    Ok(Target {
        backend: "viktor".into(),
        base_url,
        api_url,
        api_key,
        models: vec![model.clone()],
        model,
        viktor: true,
    })
}

async fn local_target(backend: Option<&str>, model: Option<&str>) -> anyhow::Result<Target> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .context("build HTTP client")?;
    let api_key = std::env::var("VKTR_LAUNCH_API_KEY")
        .ok()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| LOCAL_API_KEY.to_owned());

    // (label, root URL) candidates in probe order
    let mut candidates: Vec<(String, String)> = Vec::new();
    match backend {
        Some(b) if b.starts_with("http://") || b.starts_with("https://") => {
            candidates.push(("custom".into(), b.to_owned()));
        }
        Some(name) => {
            if let Some(url) = env_backend_url(name) {
                candidates.push((name.to_owned(), url));
            }
            match BACKENDS.iter().find(|(n, _)| *n == name) {
                Some((_, urls)) => {
                    candidates.extend(urls.iter().map(|u| (name.to_owned(), (*u).to_owned())))
                }
                None if candidates.is_empty() => bail!(
                    "unknown backend `{name}`. Known: {}. A URL also works: --backend http://host:port",
                    BACKENDS
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => {}
            }
        }
        None => {
            if let Ok(url) = std::env::var("VKTR_LAUNCH_BACKEND_URL") {
                candidates.push(("custom".into(), url));
            }
            for (name, urls) in BACKENDS {
                if let Some(url) = env_backend_url(name) {
                    candidates.push(((*name).to_owned(), url));
                }
                candidates.extend(urls.iter().map(|u| ((*name).to_owned(), (*u).to_owned())));
            }
        }
    }

    let mut failures = Vec::new();
    for (label, root) in candidates {
        let (base_url, api_url) = split_root(&root);
        match fetch_models(&client, &api_url, &api_key).await {
            Ok(models) => {
                let model = match model {
                    Some(m) => m.to_owned(),
                    None => select_model(&models).ok_or_else(|| {
                        anyhow!(
                            "`{label}` at {api_url} is up but lists no models. Load a model or pass --model <model>."
                        )
                    })?,
                };
                // A user-chosen model the backend did not list still has to appear in config-file tools' model tables.
                let mut models = models;
                if !models.iter().any(|m| m == &model) {
                    models.insert(0, model.clone());
                }
                return Ok(Target {
                    backend: label,
                    base_url,
                    api_url,
                    api_key,
                    model,
                    models,
                    viktor: false,
                });
            }
            Err(err) => failures.push(format!("{label} {api_url}: {err}")),
        }
    }
    bail!(
        "no running OpenAI-compatible backend found.\n{}\nStart one (for example `ollama serve`), pass \
         --backend <url>, or install Harbor so `vktr launch` can start a backend for you.",
        failures
            .iter()
            .map(|f| format!("  - {f}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// `VKTR_LAUNCH_<NAME>_URL` overrides a backend's well-known address.
fn env_backend_url(name: &str) -> Option<String> {
    std::env::var(format!("VKTR_LAUNCH_{}_URL", name.to_ascii_uppercase()))
        .ok()
        .filter(|u| !u.trim().is_empty())
}

/// Accept a root with or without a trailing `/v1`; return (root, root/v1).
fn split_root(root: &str) -> (String, String) {
    let trimmed = root.trim().trim_end_matches('/');
    let base = trimmed.strip_suffix("/v1").unwrap_or(trimmed).to_owned();
    let api = format!("{base}/v1");
    (base, api)
}

async fn fetch_models(
    client: &reqwest::Client,
    api_url: &str,
    api_key: &str,
) -> anyhow::Result<Vec<String>> {
    let url = format!("{api_url}/models");
    let resp = client
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() {
                anyhow!("not reachable")
            } else if e.is_timeout() {
                anyhow!("timed out")
            } else {
                anyhow!("{e}")
            }
        })?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status}");
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|_| anyhow!("/v1/models did not return JSON"))?;
    Ok(models_from_response(&body))
}

/// OpenAI-style `data[].id`, plus the Ollama-style `models[].name` / `models[].model` variant.
fn models_from_response(body: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |v: Option<&str>| {
        if let Some(id) = v.map(str::trim).filter(|s| !s.is_empty())
            && !out.iter().any(|o| o == id)
        {
            out.push(id.to_owned());
        }
    };
    for key in ["data", "models"] {
        if let Some(items) = body.get(key).and_then(|d| d.as_array()) {
            for item in items {
                match item {
                    serde_json::Value::String(s) => push(Some(s)),
                    other => push(
                        other
                            .get("id")
                            .or_else(|| other.get("name"))
                            .or_else(|| other.get("model"))
                            .and_then(|v| v.as_str()),
                    ),
                }
            }
        }
    }
    out
}

fn is_embedding_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    ["embed", "bge-", "e5-", "gte-", "rerank"]
        .iter()
        .any(|needle| id.contains(needle))
}

/// First non-embedding model, else the first model.
fn select_model(models: &[String]) -> Option<String> {
    models
        .iter()
        .find(|m| !is_embedding_model(m))
        .or_else(|| models.first())
        .cloned()
}

// ---------------------------------------------------------------------------------------------
// Per-tool wiring
// ---------------------------------------------------------------------------------------------

fn args_include_model(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "-m" || a == "--model" || a.starts_with("--model="))
}

fn pi_args_include_session(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--session-dir" | "--session" | "--continue" | "-c" | "--resume" | "-r" | "--fork"
        ) || a.starts_with("--session-dir=")
            || a.starts_with("--session=")
            || a.starts_with("--fork=")
    })
}

fn home() -> PathBuf {
    xai_dirs_home().unwrap_or_else(|| PathBuf::from("."))
}

fn xai_dirs_home() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir()
}

fn env_path(var: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or(default)
}

/// Workspace-specific pi session dir, so launching from another project never resumes this one.
fn pi_session_dir(cwd: &Path) -> PathBuf {
    let agent_dir = env_path("PI_CODING_AGENT_DIR", home().join(".pi/agent"));
    let raw = cwd.to_string_lossy();
    let mut slug = String::new();
    for ch in raw.chars() {
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-');
        let c = if keep { ch } else { '-' };
        if c == '-' && slug.ends_with('-') {
            continue;
        }
        slug.push(c);
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "workspace" } else { slug };
    env_path("VKTR_LAUNCH_PI_SESSION_ROOT", agent_dir.join("sessions")).join(slug)
}

fn plan_for(tool: &str, t: &Target, tool_args: &[String]) -> anyhow::Result<Plan> {
    let provider = format!("vktr-{}", t.backend);
    let mut plan = Plan {
        program: tool.to_owned(),
        ..Plan::default()
    };
    let has_model = args_include_model(tool_args);
    match tool {
        "claude" => {
            if !t.viktor && t.backend != "ollama" {
                bail!(
                    "claude speaks the Anthropic Messages API, which `{}` does not serve. Use --backend ollama, \
                     --viktor, or an OpenAI-compatible tool such as codex, opencode, pi or vktr.",
                    t.backend
                );
            }
            plan.env
                .insert("ANTHROPIC_BASE_URL".into(), t.base_url.clone());
            if t.viktor {
                plan.env
                    .insert("ANTHROPIC_API_KEY".into(), t.api_key.clone());
                plan.unset_env.push("ANTHROPIC_AUTH_TOKEN".into());
                // Viktor serves one model id; route Claude Code's background model slots to it too.
                for var in [
                    "ANTHROPIC_MODEL",
                    "ANTHROPIC_SMALL_FAST_MODEL",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL",
                ] {
                    plan.env.insert(var.into(), t.model.clone());
                }
                plan.env.insert(
                    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
                    "1".into(),
                );
                plan.notes.push("every Claude Code request becomes a Viktor agent run, including its background calls".into());
            } else {
                plan.env
                    .insert("ANTHROPIC_AUTH_TOKEN".into(), "ollama".into());
                plan.env.insert("ANTHROPIC_API_KEY".into(), String::new());
            }
            if !has_model {
                plan.lead_args.extend(["--model".into(), t.model.clone()]);
            }
        }
        "codex" => {
            plan.env.insert("OPENAI_API_KEY".into(), t.api_key.clone());
            let name = if t.viktor {
                "Viktor".to_owned()
            } else {
                format!("vktr {}", t.backend)
            };
            plan.lead_args.extend([
                "-c".into(),
                format!("model_providers.vktr_launch.name=\"{name}\""),
                "-c".into(),
                format!("model_providers.vktr_launch.base_url=\"{}\"", t.api_url),
                "-c".into(),
                "model_providers.vktr_launch.env_key=\"OPENAI_API_KEY\"".into(),
                "-c".into(),
                "model_providers.vktr_launch.wire_api=\"responses\"".into(),
                "-c".into(),
                "model_provider=\"vktr_launch\"".into(),
            ]);
            if !has_model {
                plan.lead_args.extend(["-m".into(), t.model.clone()]);
            }
            if matches!(t.backend.as_str(), "llamacpp") {
                plan.notes.push(
                    "Codex uses the Responses API tool schema; llama.cpp-family backends may reject it with \
                     400 'type' of tool must be 'function'. If so, use opencode or vktr with this backend."
                        .into(),
                );
            }
        }
        "copilot" => {
            plan.env
                .insert("COPILOT_PROVIDER_BASE_URL".into(), t.api_url.clone());
            plan.env
                .insert("COPILOT_PROVIDER_API_KEY".into(), t.api_key.clone());
            plan.env
                .insert("COPILOT_PROVIDER_WIRE_API".into(), "responses".into());
            plan.env.insert("COPILOT_MODEL".into(), t.model.clone());
        }
        "grok" => {
            plan.env
                .insert("GROK_MODELS_BASE_URL".into(), t.api_url.clone());
            plan.env.insert("XAI_API_KEY".into(), t.api_key.clone());
            plan.files.push((
                env_path("VKTR_LAUNCH_GROK_CONFIG", home().join(".grok/config.toml")),
                "grok model entry",
            ));
            if !has_model {
                plan.lead_args.extend(["-m".into(), provider.clone()]);
            }
        }
        "hermes" => {
            plan.env.insert("OPENAI_BASE_URL".into(), t.api_url.clone());
            plan.env.insert("OPENAI_API_KEY".into(), t.api_key.clone());
            plan.env.insert("HERMES_MODEL".into(), t.model.clone());
            if tool_args.is_empty() {
                plan.lead_args.push("chat".into());
            }
        }
        "mi" => {
            plan.env
                .insert("OPENAI_BASE_URL".into(), t.base_url.clone());
            plan.env.insert("OPENAI_API_KEY".into(), t.api_key.clone());
            plan.env.insert("MODEL".into(), t.model.clone());
        }
        "opencode" => {
            plan.env.insert("OPENAI_API_KEY".into(), t.api_key.clone());
            plan.env.insert(
                "OPENCODE_CONFIG_CONTENT".into(),
                opencode_config(&provider, t).to_string(),
            );
            if !has_model {
                plan.lead_args
                    .extend(["-m".into(), format!("{provider}/{}", t.model)]);
            }
        }
        "pi" => {
            // The provider is registered in pi's models.json and selected with pi's own flags, so the user's
            // default provider in settings.json is never changed (Harbor rewrites it; vktr does not).
            let agent_dir = env_path("PI_CODING_AGENT_DIR", home().join(".pi/agent"));
            plan.files
                .push((agent_dir.join("models.json"), "pi provider"));
            if !tool_args
                .iter()
                .any(|a| a == "--provider" || a.starts_with("--provider="))
                && !has_model
            {
                plan.lead_args.extend([
                    "--provider".into(),
                    provider.clone(),
                    "--model".into(),
                    t.model.clone(),
                ]);
            }
            if !pi_args_include_session(tool_args) {
                let cwd = std::env::current_dir().context("current directory")?;
                plan.lead_args.extend([
                    "--session-dir".into(),
                    pi_session_dir(&cwd).to_string_lossy().into_owned(),
                ]);
            }
        }
        "pool" => {
            plan.env
                .insert("POOLSIDE_STANDALONE_BASE_URL".into(), t.api_url.clone());
            plan.env
                .insert("POOLSIDE_API_KEY".into(), t.api_key.clone());
            if !has_model {
                plan.lead_args.extend(["-m".into(), t.model.clone()]);
            }
        }
        "vktr" => {
            plan.program = std::env::current_exe()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "vktr".into());
            if t.viktor {
                plan.env.insert("VIKTOR_BASE_URL".into(), t.api_url.clone());
                plan.env.insert("VIKTOR_API_KEY".into(), t.api_key.clone());
            } else {
                // Custom endpoint mode: the model list comes from the backend instead of the built-in catalog.
                plan.env
                    .insert("VKTR_MODELS_BASE_URL".into(), t.api_url.clone());
                plan.env.insert("VIKTOR_API_KEY".into(), t.api_key.clone());
                if !has_model {
                    plan.lead_args.extend(["-m".into(), t.model.clone()]);
                }
            }
        }
        other => bail!("no launch adapter for `{other}`"),
    }
    Ok(plan)
}

fn opencode_config(provider: &str, t: &Target) -> serde_json::Value {
    let ids: Vec<&String> = if t.models.is_empty() {
        vec![&t.model]
    } else {
        t.models.iter().collect()
    };
    let models: serde_json::Map<String, serde_json::Value> = ids
        .into_iter()
        .map(|id| {
            (
                id.clone(),
                serde_json::json!({"name": id, "attachment": true, "tool_call": true}),
            )
        })
        .collect();
    let name = if t.viktor {
        "Viktor".to_owned()
    } else {
        format!("vktr {}", t.backend)
    };
    serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "provider": {
            provider: {
                "npm": "@ai-sdk/openai-compatible",
                "name": name,
                "options": {"baseURL": t.api_url, "apiKey": t.api_key},
                "models": models,
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Config files
// ---------------------------------------------------------------------------------------------

fn read_json_object(path: &Path) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Default::default()),
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => Ok(map),
            Ok(_) => bail!(
                "{} is not a JSON object; refusing to overwrite it",
                path.display()
            ),
            Err(e) => bail!(
                "{} is not valid JSON ({e}); refusing to overwrite it",
                path.display()
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn write_json(path: &Path, map: serde_json::Map<String, serde_json::Value>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(map))? + "\n";
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

fn write_files(tool: &str, t: &Target, plan: &Plan) -> anyhow::Result<()> {
    let provider = format!("vktr-{}", t.backend);
    match tool {
        "pi" => {
            let Some((models_path, _)) = plan.files.first() else {
                bail!("pi launch plan is missing its config path");
            };
            let mut models = read_json_object(models_path)?;
            let providers = models
                .entry("providers")
                .or_insert_with(|| serde_json::json!({}));
            let ids: Vec<&String> = if t.models.is_empty() {
                vec![&t.model]
            } else {
                t.models.iter().collect()
            };
            if let Some(obj) = providers.as_object_mut() {
                obj.insert(
                    provider.clone(),
                    serde_json::json!({
                        "baseUrl": t.api_url,
                        "api": "openai-completions",
                        "apiKey": t.api_key,
                        "models": ids.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>(),
                    }),
                );
            } else {
                bail!("`providers` in {} is not an object", models_path.display());
            }
            write_json(models_path, models)?;
        }
        "grok" => {
            let Some((path, _)) = plan.files.first() else {
                bail!("vktr launch plan is missing its config path");
            };
            let existing = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
            };
            let mut doc: toml_edit::DocumentMut = existing.parse().with_context(|| {
                format!(
                    "{} is not valid TOML; refusing to overwrite it",
                    path.display()
                )
            })?;
            let model_tbl = doc
                .entry("model")
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut()
                .ok_or_else(|| anyhow!("[model] in {} is not a table", path.display()))?;
            model_tbl.set_implicit(true);
            let mut entry = toml_edit::Table::new();
            entry.insert("model", toml_edit::value(t.model.as_str()));
            entry.insert("base_url", toml_edit::value(t.api_url.as_str()));
            entry.insert(
                "name",
                toml_edit::value(if t.viktor {
                    "Viktor".to_owned()
                } else {
                    format!("vktr {}", t.backend)
                }),
            );
            entry.insert("env_key", toml_edit::value("XAI_API_KEY"));
            model_tbl.insert(&provider, toml_edit::Item::Table(entry));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            std::fs::write(path, doc.to_string())
                .with_context(|| format!("write {}", path.display()))?;
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Output and exec
// ---------------------------------------------------------------------------------------------

fn mask(key: &str, value: &str) -> String {
    let secret = key.contains("KEY") || key.contains("TOKEN");
    if !secret || value.is_empty() || value == LOCAL_API_KEY || value == "ollama" {
        return value.to_owned();
    }
    let head: String = value.chars().take(8).collect();
    format!("{head}… (masked)")
}

fn print_config(tool: &str, t: &Target, plan: &Plan, tool_args: &[String]) {
    println!("tool={tool}");
    println!("backend={}", t.backend);
    println!("base_url={}", t.api_url);
    println!("model={}", t.model);
    if t.models.len() > 1 {
        println!("models={}", t.models.join(","));
    }
    for (key, value) in &plan.env {
        if key == "OPENCODE_CONFIG_CONTENT" {
            // The inline config carries the API key; print it with the key masked.
            let masked = value.replace(&t.api_key, &mask("API_KEY", &t.api_key));
            println!("{key}={masked}");
        } else {
            println!("{key}={}", mask(key, value));
        }
    }
    for key in &plan.unset_env {
        println!("unset {key}");
    }
    for (path, what) in &plan.files {
        println!("wrote {} ({what})", path.display());
    }
    let mut line = vec![tool.to_owned()];
    line.extend(plan.lead_args.iter().map(|a| shell_quote(a)));
    line.extend(tool_args.iter().map(|a| shell_quote(a)));
    println!("{}", line.join(" "));
    for note in &plan.notes {
        println!("# {note}");
    }
}

fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);
    if candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())
    })
}

fn exec(plan: &Plan, tool_args: &[String]) -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new(&plan.program);
    cmd.args(&plan.lead_args).args(tool_args);
    for key in &plan.unset_env {
        cmd.env_remove(key);
    }
    cmd.envs(&plan.env);
    exec_command(cmd, &plan.program)
}

/// Replace this process with the tool (Unix), so signals, the TTY and the exit code are the tool's own.
fn exec_command(mut cmd: std::process::Command, name: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(anyhow!("failed to start `{name}`: {err}"))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .with_context(|| format!("failed to start `{name}`"))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(viktor: bool) -> Target {
        Target {
            backend: if viktor {
                "viktor".into()
            } else {
                "ollama".into()
            },
            base_url: "http://127.0.0.1:11434".into(),
            api_url: "http://127.0.0.1:11434/v1".into(),
            api_key: "sk-local".into(),
            model: "qwen3.5:4b".into(),
            models: vec!["nomic-embed-text".into(), "qwen3.5:4b".into()],
            viktor,
        }
    }

    #[test]
    fn model_discovery_reads_openai_and_ollama_shapes_and_skips_embeddings() {
        let openai = serde_json::json!({"data": [{"id": "bge-m3"}, {"id": "llama-3.1-8b"}]});
        let ollama =
            serde_json::json!({"models": [{"name": "nomic-embed-text"}, {"name": "qwen3.5:4b"}]});
        assert_eq!(
            models_from_response(&openai),
            vec!["bge-m3", "llama-3.1-8b"]
        );
        assert_eq!(
            models_from_response(&ollama),
            vec!["nomic-embed-text", "qwen3.5:4b"]
        );
        assert_eq!(
            select_model(&models_from_response(&openai)).as_deref(),
            Some("llama-3.1-8b")
        );
        assert_eq!(
            select_model(&models_from_response(&ollama)).as_deref(),
            Some("qwen3.5:4b")
        );
        assert_eq!(
            select_model(&["bge-m3".to_string()]).as_deref(),
            Some("bge-m3")
        );
    }

    #[test]
    fn split_root_accepts_both_forms() {
        assert_eq!(
            split_root("http://h:1/v1/"),
            ("http://h:1".into(), "http://h:1/v1".into())
        );
        assert_eq!(
            split_root("http://h:1"),
            ("http://h:1".into(), "http://h:1/v1".into())
        );
    }

    #[test]
    fn codex_plan_uses_a_launch_provider_and_respects_a_user_model() {
        let plan = plan_for("codex", &target(false), &[]).unwrap();
        assert_eq!(plan.env["OPENAI_API_KEY"], "sk-local");
        assert!(plan.lead_args.contains(
            &"model_providers.vktr_launch.base_url=\"http://127.0.0.1:11434/v1\"".to_string()
        ));
        assert!(
            plan.lead_args
                .ends_with(&["-m".to_string(), "qwen3.5:4b".to_string()])
        );
        let user = plan_for("codex", &target(false), &["-m".into(), "other".into()]).unwrap();
        assert!(!user.lead_args.contains(&"qwen3.5:4b".to_string()));
    }

    #[test]
    fn claude_needs_an_anthropic_speaking_target() {
        let mut t = target(false);
        t.backend = "vllm".into();
        assert!(plan_for("claude", &t, &[]).is_err());
        let viktor = plan_for("claude", &target(true), &[]).unwrap();
        assert_eq!(viktor.env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:11434");
        assert_eq!(viktor.env["ANTHROPIC_SMALL_FAST_MODEL"], "qwen3.5:4b");
    }

    #[test]
    fn pi_gets_a_workspace_session_dir_unless_the_user_picked_one() {
        let plan = plan_for("pi", &target(false), &[]).unwrap();
        assert_eq!(
            plan.lead_args[..4],
            ["--provider", "vktr-ollama", "--model", "qwen3.5:4b"]
        );
        assert_eq!(
            plan.lead_args.get(4).map(String::as_str),
            Some("--session-dir")
        );
        assert_eq!(plan.files.len(), 1, "settings.json must not be touched");
        // A user-supplied session flag suppresses only the session dir; the provider and model
        // are still wired, or pi would fall back to its own default provider.
        let user = plan_for("pi", &target(false), &["--continue".into()]).unwrap();
        assert_eq!(
            user.lead_args,
            ["--provider", "vktr-ollama", "--model", "qwen3.5:4b"]
        );
        assert_eq!(
            pi_session_dir(Path::new("/home/u/My Project"))
                .file_name()
                .unwrap(),
            "home-u-My-Project"
        );
    }

    #[test]
    fn secrets_are_masked_in_config_output() {
        assert_eq!(mask("OPENAI_API_KEY", "sk-local"), "sk-local");
        assert!(mask("VIKTOR_API_KEY", "zt_live_sk_abcdefghijkl").starts_with("zt_live_"));
        assert!(!mask("VIKTOR_API_KEY", "zt_live_sk_abcdefghijkl").contains("ijkl"));
        assert_eq!(mask("OPENAI_BASE_URL", "http://x"), "http://x");
    }
}
