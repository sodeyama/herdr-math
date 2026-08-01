# Coding-Agent Lifecycle Evidence

Date: August 1, 2026

Status: **Verified compatibility evidence, not release evidence.** The Herdr Math event handler and renderer were not implemented when this test was recorded.

## Environment

- Herdr `0.7.5`, protocol `17`
- One isolated Herdr session; the default session was not changed
- Synthetic prompts containing one `$...$` formula and one `$$...$$` formula
- A temporary event probe that recorded only event names, pane ids, workspace ids, agent ids, statuses, and field names
- No raw agent response, prompt, session id, user path, or LaTeX source was committed as evidence

## Results

| Agent | Agent version | Herdr integration | Lifecycle authority | Observed transition | Math response visible |
| --- | --- | --- | --- | --- | --- |
| Claude Code (`claude`) | `2.1.220` | `v7` | Screen detection | `idle -> working -> idle` | Yes |
| Codex (`codex`) | `0.146.0` | `v6` | Screen detection | `idle -> working -> done` | Yes |
| Pi (`pi`) | `0.83.0` | `v6` | Full lifecycle hook | `idle -> working -> done` | Yes |
| OpenCode (`opencode`) | `1.18.10` | `v9` | Full lifecycle hook | `idle -> working -> done` | Yes |

The Pi and OpenCode reports set `screen_detection_skipped` because the full lifecycle hook was authoritative. Claude Code and Codex continued to use their screen-detection manifests.

## OpenCode Isolation Check

The first OpenCode run loaded the user's normal extension set. Herdr received `working` and `done`, but an unrelated malformed skill file stopped OpenCode before it produced the requested answer.

The verification was repeated with a temporary OpenCode configuration containing only the Herdr `v9` integration. In that isolated configuration:

- `agent start` reached interactive `idle`;
- `agent prompt --wait` returned at `done`;
- the event hook received `idle -> working -> done`;
- `recent-unwrapped` pane output contained both requested delimiter forms; and
- full lifecycle hook authority remained active.

This separates Herdr integration behavior from an unrelated local extension failure. The temporary configuration was not added to the repository.

## Boundary Findings

- A completion handler cannot require `done` for every agent. Claude Code returned to `idle` after a verified response.
- Codex can complete a short response faster than a separate wait command observes `working`. Manifest event hooks still received both `working` and `done`.
- Pi and OpenCode reported stable root-session lifecycle events through their integrations.
- OpenCode's alternate-screen TUI remained readable through Herdr's `recent-unwrapped` pane source after completion.
- The event envelope included workspace id, pane id, status, and an agent hint in these runs. The exported schema permits the agent hint to be absent, so `pane.get` remains the authoritative identity check.
- The probe confirmed event delivery only. It did not render an image or enable any agent in the Herdr Math allowlist.

## Commands Used

The verification used these command families with the isolated session name and synthetic inputs:

```sh
herdr --session <test-session> server
herdr integration status
herdr api schema --output <fixture>
herdr --session <test-session> agent start <name> --kind <agent> --pane <pane>
herdr --session <test-session> agent prompt <name> <synthetic-prompt> --wait
herdr --session <test-session> agent read <name> --source recent-unwrapped
herdr --session <test-session> plugin log list --plugin <probe-id>
```

## Acceptance Mapping

This evidence satisfies task T-107's prerequisite for AT-100 and AT-112. It does not claim that AT-112 passes end to end; the scanner, boundary detector, renderer, viewer lifecycle, and release smoke test remain planned work.
