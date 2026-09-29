#!/usr/bin/env bash
# The behaviour tests behind .facts: targeted cargo tests and the mock-Viktor smoke scripts.
# .facts only checks that these exist (fast, static); this runs them. Needs a release build
# for the smoke steps (cargo build --release -p xai-grok-pager-bin).
#
#   scripts/test.sh            run everything
#   scripts/test.sh <pattern>  run only steps whose description contains <pattern>
set -u
cd "$(dirname "$0")/.."
PATTERN="${1:-}"; PASS=0; FAIL=0; FAILED=()
step() {
  local desc="$1"; shift
  [ -n "$PATTERN" ] && [[ "$desc" != *"$PATTERN"* ]] && return
  printf "== %s\n" "$desc"
  if bash -c "$1"; then PASS=$((PASS+1)); else FAIL=$((FAIL+1)); FAILED+=("$desc"); fi
}

step 'with only VIKTOR_API_KEY and VIKTOR_BASE_URL set, vktr -p chats over responses, chat completions and' scripts/smoke-m1.sh
step 'a failed headless run prints its error once, as plain text (the agent protocols JSON quoting is unw' 'cargo test -p xai-grok-pager --lib -- readable_error_tests && grep -q '"'"'ErrorAlreadyReported'"'"' crates/codegen/xai-grok-pager-bin/src/main.rs'
step 'starting the TUI without a key (or /login) opens a sign-in screen that takes a pasted Viktor API key' 'cargo test -p xai-grok-pager --lib -- login_is_a_viktor_key_paste_box && grep -q '"'"'set_signed_in_api_key'"'"' crates/codegen/xai-grok-login/src/auth_method.rs'
step 'for the built-in viktor model VIKTOR_API_KEY in the environment wins over the key saved by vktr logi' 'cargo test -p xai-grok-shell --lib -- own_credential'
step 'caller tools (read_file, run_terminal_command, search_replace, write) run locally and their results ' scripts/smoke-m2.sh
step 'the main agents toolset is lean by default: workflows and subagents are off at their source and the' 'cargo test -p xai-grok-shell --lib -- the_lean_toolset_denies subagents_config_default_follows background_workflows_default_follows'
step 'sessions persist under VKTR_HOME/sessions/<encoded cwd>/; --continue and --resume <id> reload them i' scripts/smoke-m3.sh
step 'a turn sent right after a cancelled one meets Viktors HTTP 409 conversation_busy for about 15 s; th' 'cargo test -p xai-grok-sampling-types --lib -- a_busy_viktor_thread_is_retried'
step 'automatic compaction is skipped for the built-in viktor model on the Responses backend with continua' 'cargo test -p xai-grok-shell --lib -- viktor_context_tests'
step 'vktr launch [--backend <svc|url>] [--model <m>] [--config] <tool> [args] finds a reachable OpenAI-co' scripts/smoke-m4.sh
step 'the logo is the Viktor wordmark from the Viktor decks, rasterized to 57x14 and 46x11 pixel grids (1.' 'cargo test -p xai-grok-pager --lib -- views::welcome::logo welcome_parks_once_the_wordmark_settles'
step 'no help screen names grok or x.ai: scripts/check-help-text.sh walks vktr --help and every subcommand' 'test -x target/release/vktr && scripts/check-help-text.sh target/release/vktr'
step 'vktr doctor ends with a Viktor section: endpoint and its source, the keys source shown as zt_live_s' 'cargo test -p xai-grok-pager-bin -- a_masked_key_shows_only'
step 'vktr -p reads piped input: with a prompt it is appended as a <stdin> block, a bare -p takes it as th' 'cargo test -p xai-grok-pager --lib -- piped_stdin_tests'
step '/privacy states where vktr sends data (only the configured model endpoint; nothing to xAI: no teleme' 'cargo test -p xai-grok-pager --lib -- privacy_states_where_data_goes the_xai_training_opt_in_is_not_a_vktr_setting'
step 'requests identify as vktr/<version> (<os>; <arch>): the TUIs and headless modes internal client na' 'cargo test -p xai-grok-sampler --lib -- vktrs_own_front_ends_are_just_vktr && cargo test -p xai-grok-http --lib -- vktrs_own_tui_is_just_vktr'
step 'vktr acp serves Viktor to ACP editors over stdio and keeps wire parity with the TypeScript viktor-ac' 'cargo test -p vktr-acp'
step 'the shipped binary serves ACP end to end: scripts/smoke-acp.py drives `vktr acp` over stdio against ' 'scripts/smoke-acp.py target/release/vktr'
step 'when the editor offers fs or terminal capabilities, vktr acp gives Viktor matching caller tools (edi' 'cargo test -p vktr-acp --test acp_protocol -- a_chat_only_client_gets_no_tools a_read_runs_through_the_editor a_write_asks_the_user_with_its_diff a_rejected_write_leaves a_command_runs_in_an_editor_terminal an_editor_that_cannot_ask always_allow_is_remembered a_call_to_a_tool_the_editor_does_not_offer cancelling_at_the_permission_prompt the_tools_can_be_turned_off'
step 'writes and commands ask session/request_permission first (allow once, always allow in this session, ' 'cargo test -p vktr-acp --test acp_protocol -- a_write_asks_the_user a_rejected_write an_editor_that_cannot_ask always_allow'
step 'vktr acp keeps each session in $VKTR_HOME/acp/sessions/<id>.json (owner-only; thread id, workspace, ' 'cargo test -p vktr-acp -- a_restarted_agent_loads_a_session resume_restores_the_thread a_saved_session_loads_back an_id_that_is_not_a_plain_file_name'
step 'after a cancelled turn Viktor answers the thread with HTTP 409 conversation_busy for about 15 s whil' 'cargo test -p vktr-acp --test acp_protocol -- a_prompt_right_after_a_cancel_waits cancelling_at_the_permission_prompt'
step 'a tool output is owed to Viktor from the moment it exists until Viktor accepts a request carrying it' 'cargo test -p vktr-acp --test acp_protocol -- tool_outputs_survive_a_failed_request owed_tool_outputs_are_stored a_prompt_that_supersedes_a_running_one'
step 'editor tool paths are resolved against the workspace root and collapsed lexically (. and ..) before ' 'cargo test -p vktr-acp -- dot_dot_is_collapsed a_read_outside_the_workspace_asks_first'
step 'the MCP servers an editor lists in session/new (stdio and HTTP) are connected on the users machine ' 'cargo test -p vktr-acp -- an_editor_mcp_server_is_offered a_rejected_or_failing_mcp_call an_mcp_server_that_cannot_start mcp_servers_can_be_turned_off names_are_valid_function_names a_combinator_schema'
step 'vktr acp --print-config zed|jetbrains prints the editors agent_servers snippet with the path of the' 'test -x target/release/vktr && target/release/vktr acp --print-config zed 2>/dev/null | python3 -c '"'"'import json,sys; d=json.load(sys.stdin); assert d["agent_servers"]["Viktor"]["args"]==["acp"]'"'"''

echo "$PASS passed, $FAIL failed"
for f in "${FAILED[@]}"; do echo "  failed: $f"; done
[ "$FAIL" -eq 0 ]
