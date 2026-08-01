# Final-response Presentation Runtime Evidence

Date: August 2, 2026

## Scope

This evidence covers the implementation through commit `40a79a9` in an isolated Herdr session attached to Ghostty.
It records bounded outcome and pane metadata only. No prompt, response, LaTeX source, agent session value, local path,
or screenshot is included.

## Environment

- Herdr: 0.7.5, protocol 17
- Herdr Math: 0.1.0 development build
- Ghostty: 1.3.1 stable
- Platform: macOS 26.5.2 on arm64
- Measured terminal cell: 7 by 15 pixels
- Herdr integrations: Claude Code v7, Codex v6, Pi v6, OpenCode v9

## Real-agent result

| Agent | Lifecycle authority | Final-response result | Viewer result |
|---|---|---|---|
| Claude Code | Screen detection | A concise tool-using turn produced `baseline_stored` then `image_published` | One owned viewer was created; a later unprovable turn failed closed without removing it |
| Codex CLI | Screen detection | A prose, inline-math, and display-math turn produced `image_published` | One owned viewer was created without taking source focus |
| Pi | Integration hook | Concise and long tool-using turns produced `image_published` after conclusion-only extraction | The same viewer was updated; close cleanup and recreation passed |
| OpenCode | Integration hook | A normal `working -> done` turn produced `baseline_stored` then `image_published` | One owned viewer was created from a fixed-header suffix proof |

OpenCode exposed both fixed-footer prefix and fixed-header suffix replacement layouts across real turns. The public
implementation now stores bounded formula HMACs on eligible anchors and accepts either layout only when the anchor is
unique at the same line-from-bottom position, both contexts match, a new formula is proven, and textual tool and
completion boundaries enclose the final response. Synthetic unit and socket integration cases cover both layouts and
their failure conditions.

Claude Code passed for a concise response that remained visible. A prior long response lost its leading rows from the
alternate screen and was rejected because the complete final boundary was unavailable. This is the required fail-closed
behavior. Herdr documents that alternate-screen rows from Claude Code and OpenCode can disappear and cannot be recovered
by requesting more lines. See [Agent Automation](https://herdr.dev/docs/agent-automation/) and
[Socket API](https://herdr.dev/docs/socket-api/).

## Presentation and viewer checks

| Check | Evidence | Result |
|---|---|---|
| Conclusion-only prose and math | Per-agent final-response extraction plus real successful publish | Passed programmatically for all four agents |
| Transparent background | Renderer alpha-channel test on the same build | Passed |
| Matching prose and TeX size | Renderer CSS and image test using one inherited 20 px base size | Passed |
| Long response | Real Pi response rendered at 480 by 4,318 pixels and compressed to 210,119 bytes | Passed transport and presentation execution |
| Automatic scroll and bottom frame | Managed-viewer crop-frame integration tests plus the real long-response presenter path | Passed programmatically |
| Viewer reuse | Real Pi update retained one owned viewer | Passed |
| Focus preservation | Source focus metadata was unchanged on create and update | Passed |
| Resize behavior | A resized alternate-screen snapshot failed closed when its old boundary became unprovable; current geometry is covered by integration tests | Passed safety behavior |
| Rollback preservation | A later real Claude boundary failure retained the previous owned viewer and mapping | Passed |
| Close and recreation | Real Pi viewer closure cleared its mapping and the next valid turn created one new viewer | Passed |

The available Computer Use safety policy did not permit direct inspection of Ghostty. Therefore transparent compositing,
matched visual size, animation smoothness, and final resting position have programmatic evidence but not a direct UI
observation in this run. T-807 remains open until that visual check is completed. T-903 remains open for a new sanitized
public screenshot.

## Final automated validation

```sh
npm run check
npm test
npm run test:integration
npm run build
npm run smoke:render
```

- Complete suite: 43 files, 333 tests passed
- Integration suite: 20 files, 134 tests passed
- Renderer smoke: 1 file, 7 tests passed
- Manifest, type, lint, format, runtime dependency, and security gates passed
- Security scan: 45 runtime files and 284 release files passed

## Acceptance status

- AT-212 and AT-213 passed with unit, socket integration, and real OpenCode evidence.
- AT-309 conclusion-only extraction passed for concise real turns from all four agents.
- AT-400 and AT-412 retain automated renderer evidence on the runtime build.
- Viewer ownership, reuse, focus, rollback, closure, recreation, and long-image transport have real Herdr evidence.
- T-807 is not complete because direct Ghostty visual inspection remains unavailable.
