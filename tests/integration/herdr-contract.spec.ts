import { readFileSync } from "node:fs";

import { Ajv2020, type ValidateFunction } from "ajv/dist/2020.js";
import { describe, expect, it } from "vitest";

const fixtureRoot = new URL("../fixtures/herdr/", import.meta.url);

interface HerdrSchema {
  schema_version: number;
  protocol: number;
  schemas: {
    success_response: {
      $defs: {
        InstalledPluginInfo: { required: string[] };
        PluginPlatform: { enum: string[] };
      };
    };
  };
}

function readJson<T>(name: string): T {
  return JSON.parse(readFileSync(new URL(name, fixtureRoot), "utf8")) as T;
}

function requireValidator(validator: ValidateFunction | undefined): ValidateFunction {
  if (validator === undefined) {
    throw new Error("Expected schema validator was not registered");
  }
  return validator;
}

const schema = readJson<HerdrSchema>("api-schema-0.7.5.json");
const events = readJson<unknown[]>("agent-status-events.json");
const paneResponses = readJson<unknown[]>("pane-info-responses.json");
const integrationContract = readJson<{
  herdrVersion: string;
  protocol: number;
  minimumHerdrVersion: string;
  agents: Array<{
    id: string;
    minimumIntegrationVersion: number;
    lifecycleAuthority: string;
  }>;
}>("agent-integration-contract.json");

const ajv = new Ajv2020({
  strict: false,
  formats: {
    float: true,
    int32: true,
    uint: true,
    uint16: true,
    uint32: true,
    uint64: true
  }
});
ajv.addSchema(schema, "herdr-api");

describe("Herdr 0.7.5 public contract", () => {
  it("pins the exported protocol schema", () => {
    expect(schema.schema_version).toBe(1);
    expect(schema.protocol).toBe(17);
    expect(integrationContract.herdrVersion).toBe("0.7.5");
    expect(integrationContract.minimumHerdrVersion).toBe("0.7.5");
    expect(integrationContract.protocol).toBe(17);
  });

  it("accepts status events for every supported coding agent", () => {
    const validate = requireValidator(ajv.getSchema("herdr-api#/schemas/event"));

    for (const event of events) {
      expect(validate(event), JSON.stringify(validate.errors)).toBe(true);
    }

    expect(integrationContract.agents.map(({ id }) => id)).toEqual(["claude", "codex", "pi", "opencode"]);
  });

  it("allows an event without an agent hint", () => {
    const validate = requireValidator(ajv.getSchema("herdr-api#/schemas/event"));
    const event = {
      event: "pane_agent_status_changed",
      data: {
        type: "pane_agent_status_changed",
        pane_id: "w1:p5",
        workspace_id: "w1",
        agent_status: "working"
      }
    };

    expect(validate(event), JSON.stringify(validate.errors)).toBe(true);
  });

  it("accepts authoritative pane.get responses", () => {
    const validate = requireValidator(ajv.getSchema("herdr-api#/schemas/success_response"));

    for (const response of paneResponses) {
      expect(validate(response), JSON.stringify(validate.errors)).toBe(true);
    }
  });

  it("exposes required plugin and platform fields", () => {
    const definitions = schema.schemas.success_response.$defs;
    const required = definitions.InstalledPluginInfo.required;
    const platforms = definitions.PluginPlatform.enum;

    expect(required).toEqual(
      expect.arrayContaining(["plugin_id", "name", "version", "manifest_path", "plugin_root", "enabled"])
    );
    expect(platforms).toEqual(["linux", "macos", "windows"]);
  });

  it("contains no local home paths in public fixtures", () => {
    for (const name of ["agent-status-events.json", "pane-info-responses.json", "agent-integration-contract.json"]) {
      const fixture = readFileSync(new URL(name, fixtureRoot), "utf8");
      expect(fixture).not.toContain("/Users/");
      expect(fixture).not.toContain("/home/");
    }
  });
});
