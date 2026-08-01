# Herdr Session Restart Evidence

Date: 2026-08-01

## Scope

This test used a dedicated named Herdr 0.7.5 session. The default session remained running throughout the test.
Only fingerprint state, stable plugin outcomes, file modes, and pane ownership metadata were inspected. Raw pane
content, LaTeX source, agent session values, and local paths are not part of this evidence.

## Session isolation

The default and named sessions both used a source pane with the same local pane ordinal. Herdr Math derived two
different session keys and stored their pane fingerprints in separate digest directories. The named state changes
did not alter any default-session fingerprint file.

The default session reported 12 workspaces and 20 panes before the named restart. It reported the same counts,
protocol 17, and a running server after restart recovery and after the named server was stopped.

## Stale-lock and restart procedure

1. A temporary client attached to the named server and a valid completion created one owned viewer.
2. The named server was stopped without stopping the default server.
3. The production lock API created a synthetic dead lock older than the 120-second stale threshold for the named
   source state. The file mode was `0600`, the process id was not live, and no raw content was stored.
4. The same named server was restarted.
5. The one-shot startup hook exited successfully with `stale_locks: 1`, `expired_states: 0`, and
   `stale_temporary_files: 0`.
6. The named workspace and its previous viewer pane were restored by Herdr. The restored pane no longer carried
   authoritative Herdr Math ownership metadata, so the plugin did not modify or close it.
7. A real Pi process was started after restart. Its initial settled event returned `baseline_missing`, as required
   without a new working baseline. The next real prompt produced `baseline_stored` followed by `image_published`.
8. Safe ownership recovery created one new metadata-verified viewer and preserved source focus.
9. The named server was stopped. The default server remained running and unchanged.

The restored unowned pane was deliberately preserved. Treating its old id as proof of ownership would violate the
viewer ownership invariant after a server restart. The new viewer was the only pane counted as owned by Herdr Math.

## One-shot behavior

The startup log contained one completed cleanup invocation for the restart. It did not launch a controller or
leave a cleanup process running. Later event hooks executed as separate short-lived processes and completed
normally.

## Acceptance result

- AT-600 passed: equal pane ordinals in default and named sessions produced separate state namespaces.
- AT-602 passed: the dead, old, valid lock was removed and later processing resumed.
- AT-606 passed: startup cleanup ran once, reported bounded counts, and exited.
- AT-607 passed: owned-viewer state existed before stop, conservative ownership recovery occurred after restart,
  a real Pi completion rendered successfully, and the default session was unaffected.

