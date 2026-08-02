# Post-V2 Backlog

Items managed after the `0.2.0` release. P1 items are required before claiming the associated
capability; P2 items are exploration. None of these are `0.2.0` commitments. A capability is not
described as working until it has its own specification and compatibility evidence.

## P1

- **Additional terminals** — run the Ghostty matrix (`AT-2-700`) in kitty and WezTerm; a terminal
  is listed as verified only after the same matrix passes (`AT-2-701`).
- **Linux** — complete the install, build, native-dependency, and render matrix before adding any
  Linux platform claim.
- **`tmath watch <file>`** — re-render on file change with debounced bounded reads.

## P2

- **Windows** — named pipes, build commands, renderer native dependencies, and terminal graphics
  must pass before any Windows claim.
- **Shared-memory/file media** — optional `t=s`/`t=f` Kitty media to avoid pushing large
  payloads through a pipe, keeping the bounded-size invariants.
- **Accessible and alternate output** — accessible equation text, copy actions, MathML, and
  non-image fallbacks without silently changing `0.2.0` behavior.
- **Placement sizing controls** — user-configurable placement and foreground color directives
  (require a threat-model and specification update first).

## Labeling rule

Anything in this document is planned or unsupported. Do not present it as released, working, or
verified in user-facing documentation until the corresponding acceptance tests and runtime
evidence exist.
