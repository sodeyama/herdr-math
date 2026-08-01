import { describe, expect, it } from "vitest";

import { failure, success } from "../../src/core/contracts.js";
import {
  createDiagnosticReport,
  createGraphicsChecks,
  createVersionCheck,
  diagnosticCheck,
  parseDiagnosticEnvironment
} from "../../src/diagnostics/model.js";
import { VIEWER_IDENTITY } from "../../src/viewer/ownership.js";

describe("diagnostic report model", () => {
  it("extracts only bounded authoritative action context", () => {
    const secret = "SECRET_SELECTED_TEXT_AND_LOCAL_PATH";
    const parsed = parseDiagnosticEnvironment({
      HERDR_SOCKET_PATH: "/tmp/herdr.sock",
      HERDR_BIN_PATH: "/usr/local/bin/herdr",
      HERDR_PLUGIN_ID: VIEWER_IDENTITY.pluginId,
      HERDR_PLUGIN_ROOT: "/opt/herdr-math",
      HERDR_PLUGIN_CONFIG_DIR: "/tmp/herdr-math-config",
      HERDR_PLUGIN_STATE_DIR: "/tmp/herdr-math-state",
      HERDR_PLUGIN_CONTEXT_JSON: JSON.stringify({
        focused_pane_id: "w1:p1",
        workspace_id: "w1",
        selected_text: secret,
        focused_pane_cwd: secret,
        unknown_future_field: secret
      })
    });

    expect(parsed).toEqual({
      socketPath: "/tmp/herdr.sock",
      configDirectory: "/tmp/herdr-math-config",
      stateDirectory: "/tmp/herdr-math-state",
      sourcePaneId: "w1:p1",
      workspaceId: "w1"
    });
    expect(JSON.stringify(parsed)).not.toContain(secret);
  });

  it("rejects malformed, oversized, and non-Herdr action environments", () => {
    expect(parseDiagnosticEnvironment({})).toBeNull();
    expect(
      parseDiagnosticEnvironment({
        HERDR_SOCKET_PATH: "/tmp/herdr.sock",
        HERDR_BIN_PATH: "/usr/local/bin/herdr",
        HERDR_PLUGIN_ID: "other.plugin",
        HERDR_PLUGIN_ROOT: "/opt/herdr-math",
        HERDR_PLUGIN_CONFIG_DIR: "/tmp/herdr-math-config",
        HERDR_PLUGIN_STATE_DIR: "/tmp/herdr-math-state",
        HERDR_PLUGIN_CONTEXT_JSON: JSON.stringify({ focused_pane_id: "../pane", workspace_id: "w1" })
      })
    ).toBeNull();
    expect(
      parseDiagnosticEnvironment({
        HERDR_SOCKET_PATH: "/tmp/herdr.sock",
        HERDR_BIN_PATH: "/usr/local/bin/herdr",
        HERDR_PLUGIN_ID: VIEWER_IDENTITY.pluginId,
        HERDR_PLUGIN_ROOT: "/opt/herdr-math",
        HERDR_PLUGIN_CONFIG_DIR: "/tmp/herdr-math-config",
        HERDR_PLUGIN_STATE_DIR: "/tmp/herdr-math-state",
        HERDR_PLUGIN_CONTEXT_JSON: "x".repeat(65 * 1024)
      })
    ).toBeNull();
  });

  it.each([
    ["0.7.4", 17, "herdr_version_unsupported"],
    ["0.7.5-beta.1", 17, "herdr_version_unsupported"],
    ["0.7.5", 18, "herdr_protocol_unsupported"],
    ["0.7.5", 17, "herdr_version_ok"],
    ["0.8.0", 17, "herdr_version_ok"]
  ])("classifies Herdr %s protocol %i", (version, protocol, code) => {
    expect(createVersionCheck(success({ version, protocol }))).toMatchObject({ code });
  });

  it("keeps graphics remediation and report fields allowlisted", () => {
    const disabled = createGraphicsChecks(
      failure({ code: "graphics_disabled", retryable: false, details: { actual: 123 } })
    );
    expect(disabled[0]).toEqual({
      id: "graphics",
      status: "fail",
      code: "graphics_disabled",
      message: "Herdr experimental Kitty graphics are disabled.",
      action: "Set [experimental].kitty_graphics = true in Herdr config, then run herdr server reload-config."
    });
    expect(createGraphicsChecks(success({ cellWidthPx: 0, cellHeightPx: 16 }))[1]).toMatchObject({
      code: "cell_size_unavailable"
    });
    expect(createDiagnosticReport([diagnosticCheck("environment", "pass", "environment_ok", "Safe message")])).toEqual({
      schemaVersion: 1,
      plugin: "Herdr Math",
      pluginVersion: "0.1.0",
      minimumHerdrVersion: "0.7.5",
      expectedHerdrProtocol: 17,
      outcome: "ok",
      checks: [{ id: "environment", status: "pass", code: "environment_ok", message: "Safe message" }]
    });
  });
});
