//! `/privacy`: say where vktr sends your data.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Say where vktr sends data. Upstream opened xAI's training opt-in here; vktr has no such
/// setting because nothing goes to xAI. Takes no arguments.
pub struct PrivacyCommand;

/// What `/privacy` prints. Every line is a fact the spec checks (`.facts`, sections domain and m5).
pub(crate) const PRIVACY_STATEMENT: &str = "\
Where vktr sends your data:
  • Prompts, files you attach, and tool results go only to the model endpoint you configured
    (Viktor at VIKTOR_BASE_URL, or a [model.*] you added). Viktor's own data handling applies there.
  • Nothing goes to xAI: no telemetry, no training opt-in, no self-update, no session sharing,
    no trace upload. /feedback and `vktr trace` stay on this machine.
  • Sessions, logs and your saved API key live under ~/.vktr (VKTR_HOME); the key file is owner-only.";

impl SlashCommand for PrivacyCommand {
    slash_meta! {
        name: "privacy",
        description: "Show where vktr sends your data",
        usage: "/privacy",
    }

    /// Trailing text is ignored, not rejected: `/privacy opt-in` from muscle memory should still answer.
    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Message(PRIVACY_STATEMENT.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `/privacy <args>` in `mode`.
    fn run_privacy(args: &str, mode: crate::app::ScreenMode) -> CommandResult {
        use crate::acp::model_state::ModelState;
        use crate::app::bundle::BundleState;

        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: mode,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        };
        PrivacyCommand.run(&mut ctx, args)
    }

    fn states_where_data_goes(result: &CommandResult) -> bool {
        matches!(result, CommandResult::Message(text) if text == PRIVACY_STATEMENT)
    }

    /// Every screen mode gets the statement; there is no xAI settings page to open.
    #[test]
    fn privacy_states_where_data_goes_in_every_screen_mode() {
        use crate::app::ScreenMode;
        for mode in [
            ScreenMode::Fullscreen,
            ScreenMode::Inline,
            ScreenMode::Minimal,
        ] {
            let result = run_privacy("", mode);
            assert!(
                states_where_data_goes(&result),
                "`/privacy` in {mode:?} must print the statement, got {result:?}",
            );
        }
        assert!(PRIVACY_STATEMENT.contains("Nothing goes to xAI"));
    }

    /// The arguments this used to accept must not linger as hidden aliases that change a privacy preference straight from the prompt.
    #[test]
    fn arguments_are_ignored_not_honored() {
        use crate::app::ScreenMode;
        assert!(
            !PrivacyCommand.takes_args(),
            "the dropdown must not offer an argument slot"
        );
        for args in [
            "   ", "opt-in", "opt-out", "in", "out", "share", "private", "status", "info",
            "garbage",
        ] {
            let result = run_privacy(args, ScreenMode::Inline);
            assert!(
                states_where_data_goes(&result),
                "`/privacy {args}` must just open the page, got {result:?}",
            );
        }
    }
}
