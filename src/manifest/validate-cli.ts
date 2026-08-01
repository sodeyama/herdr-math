import { readFileSync } from "node:fs";

import { validateManifest } from "./validate.js";

interface ContractFixture {
  minimumHerdrVersion: string;
}

interface HerdrSchemaFixture {
  schemas: {
    event: { $defs: { EventKind: { enum: string[] } } };
    success_response: { $defs: { PluginPlatform: { enum: string[] } } };
  };
}

interface PackageMetadata {
  version: string;
}

function readJson<T>(path: string): T {
  return JSON.parse(readFileSync(path, "utf8")) as T;
}

const schema = readJson<HerdrSchemaFixture>("tests/fixtures/herdr/api-schema-0.7.5.json");
const contract = readJson<ContractFixture>("tests/fixtures/herdr/agent-integration-contract.json");
const packageMetadata = readJson<PackageMetadata>("package.json");

const errors = validateManifest({
  manifestSource: readFileSync("herdr-plugin.toml", "utf8"),
  packageVersion: packageMetadata.version,
  minimumHerdrVersion: contract.minimumHerdrVersion,
  eventKinds: schema.schemas.event.$defs.EventKind.enum,
  pluginPlatforms: schema.schemas.success_response.$defs.PluginPlatform.enum,
  sourceEntrypoints: [
    "src/startup.ts",
    "src/on-agent-status.ts",
    "src/on-pane-closed.ts",
    "src/diagnose.ts",
    "src/viewer.ts"
  ]
});

if (errors.length > 0) {
  for (const error of errors) {
    process.stderr.write(`manifest validation: ${error}\n`);
  }
  process.exitCode = 1;
} else {
  process.stdout.write("Herdr plugin manifest validation passed.\n");
}
