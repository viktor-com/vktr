# Recordings

Terminal recordings (`asciinema play <file>`), newest first. None contains key material: every
cast is checked for the key when it is made.

## Live Viktor

Recorded with `scripts/record-live.py <flow>` against `https://api.viktor.com/api/compat/v1`.

| File | Flow |
| --- | --- |
| `live-signin.cast` | First run with no key: `vktr` opens the sign-in screen, the pasted key (masked) is checked and saved, the first prompt is answered, and `vktr doctor` shows the saved key |
| `live-login.cast` | `vktr login --api-key`, then `unset VIKTOR_API_KEY` and a prompt that runs on the saved key |
| `live-coding.cast` | A project with a failing test: Viktor runs the tests, fixes `calc.py`, re-runs them; `git diff` and a green run in the shell afterwards |
| `live-approval.cast` | Viktor wants to write `README.md`, the write is rejected and never happens; the redirected `CALC.md` is allowed for the session and written |
| `live-acp-launch.cast` | `vktr acp` launched as an editor would (`scripts/acp-demo.py`): reads, a write and a command through the editor with permission prompts, then an agent restart, `session/load` and a follow-up on the same Viktor thread |
| `m1-live-viktor.cast` | TUI against live Viktor: a local read tool and an explanation |

The README's `docs/assets/demo-coding.gif` is rendered from `live-coding.cast` with
[agg](https://github.com/asciinema/agg):

```
agg --font-size 14 --idle-time-limit 2 --speed 1.2 docs/recordings/live-coding.cast docs/assets/demo-coding.gif
```

## Mock server

Recorded with `scripts/record-tui.sh` against `scripts/mock_viktor.py`.

| File | Flow |
| --- | --- |
| `m1-chat.cast`, `m1-run-failed.cast`, `m1-empty-reply.cast` | Streaming, a failed run and an empty reply |
| `m2-edit-approval.cast` | Edit diff with the four-way approval prompt |
| `m3-slash-and-resume.cast` | Slash commands and resume |
| `m4-launch.cast` | `vktr launch` wiring |
| `welcome/` | The welcome screen at several terminal sizes, day and night themes |
