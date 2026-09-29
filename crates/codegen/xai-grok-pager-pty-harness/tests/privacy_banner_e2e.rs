//! E2E: the upstream coding-data privacy banner never shows in vktr.
//! Upstream showed it to opted-out OAuth users under the `privacy_notice_rollout` flag and asked them to share
//! coding data with the vendor. vktr sends nothing to xAI, so the flag is pinned off: neither
//! `VKTR_PRIVACY_NOTICE_ROLLOUT=1` nor remote settings bring the banner back.
//!
//! Drives the real pager binary through a PTY against the shared mock inference server (isolated `$HOME`).
//! A seeded opted-out OAuth entry is the active auth (`XAI_API_KEY` removed), which is exactly the account the banner targeted.
//!
//! ```bash
//! cargo test -p xai-grok-pager-pty-harness --test privacy_banner_e2e \
//!   -- --ignored --nocapture
//! ```

use std::time::Duration;

use anyhow::{Context, Result};
use xai_grok_pager_pty_harness::{
    ContentController, EnvOp, PtyHarness, pager_binary, seed_fake_oauth_coding_data_opted_out,
};

const ROWS: u16 = 50;
const COLS: u16 = 120;
const BANNER_TITLE: &str = "Help improve vktr";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore] // opt-in: spawns the real pager binary in a PTY (CI runs with --ignored)
async fn privacy_banner_stays_hidden_even_when_forced_on() {
    run().await.expect("privacy banner e2e");
}

async fn run() -> Result<()> {
    let content = ContentController::start()
        .await
        .context("start mock server")?;
    seed_fake_oauth_coding_data_opted_out(&content, "pty-privacy-user");

    let project = tempfile::tempdir().context("project dir")?;
    std::fs::create_dir_all(project.path().join(".git")).context("create .git")?;
    let binary = pager_binary().context("resolve pager binary")?;

    let mut pager = PtyHarness::spawn_with_content_env_ops_in_dir(
        &binary,
        ROWS,
        COLS,
        &content,
        &[],
        &[
            EnvOp::set("VKTR_PRIVACY_NOTICE_ROLLOUT", "1"),
            EnvOp::remove("XAI_API_KEY"),
        ],
        Some(project.path()),
    )
    .context("spawn pager")?;

    // "New worktree" renders only on the authenticated welcome menu, where upstream showed the banner.
    pager
        .wait_for_text("New worktree", Duration::from_secs(20))
        .context("authenticated welcome screen")?;
    pager.update(Duration::from_secs(2));
    assert!(
        !pager.contains_text(BANNER_TITLE),
        "privacy banner showed despite being pinned off:\n{}",
        pager.screen_contents()
    );
    Ok(())
}
