// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// The wordmark is drawn with the half-block elements `▀ ▄` (U+2580, U+2584). A writer-thread
/// regression (`WriteFile` instead of `WriteConsoleW` on Windows, or a missing
/// `SetConsoleOutputCP(65001)`) garbles the output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn welcome_screen_wordmark_renders_correctly() {
    let content = ContentController::start().await.expect("start content");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    let screen = harness.screen_contents();

    // Only the wordmark uses these glyphs; no menu label or border does.
    for glyph in ['▀', '▄'] {
        assert!(
            screen.contains(glyph),
            "{glyph:?} not found in screen — the wordmark may be garbled by code-page \
             misinterpretation.\nScreen contents:\n{screen}"
        );
    }

    harness.quit().expect("clean quit");
}
