import { isAbsolute } from "node:path";

export interface ManifestValidationInput {
  manifestSource: string;
  packageVersion: string;
  minimumHerdrVersion: string;
  eventKinds: string[];
  pluginPlatforms: string[];
  sourceEntrypoints: string[];
}

interface ManifestSection {
  command: string[];
  id: string | undefined;
  on: string | undefined;
}

interface ManifestSnapshot {
  id: string;
  name: string;
  version: string;
  minimumHerdrVersion: string;
  description: string;
  platforms: string[];
  build: ManifestSection[];
  startup: ManifestSection[];
  events: ManifestSection[];
  actions: ManifestSection[];
  panes: ManifestSection[];
}

const EXPECTED_ID = "io.github.sodeyama.herdr-math";
const EXPECTED_NAME = "Herdr Math";
const EXPECTED_DESCRIPTION = "Render LaTeX from AI agent responses in a side pane.";
const REQUIRED_EVENTS = ["pane.agent_status_changed", "pane.closed"];
const VERIFIED_PLATFORMS = ["macos"];
const FORBIDDEN_COMMAND_FRAGMENTS = ["prototype", "herdr-latex", "docs/obsidian"];

function escapePattern(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function readString(source: string, key: string): string {
  const match = new RegExp(`^${escapePattern(key)}\\s*=\\s*"([^"]*)"$`, "m").exec(source);
  if (match?.[1] === undefined) {
    throw new Error(`Missing string field: ${key}`);
  }
  return match[1];
}

function readStringArray(source: string, key: string): string[] {
  const match = new RegExp(`^${escapePattern(key)}\\s*=\\s*(\\[[^\\n]*\\])$`, "m").exec(source);
  if (match?.[1] === undefined) {
    throw new Error(`Missing array field: ${key}`);
  }

  const parsed: unknown = JSON.parse(match[1]);
  if (!Array.isArray(parsed) || !parsed.every((value) => typeof value === "string")) {
    throw new Error(`Invalid string array: ${key}`);
  }
  return parsed;
}

function readOptionalString(source: string, key: string): string | undefined {
  const match = new RegExp(`^${escapePattern(key)}\\s*=\\s*"([^"]*)"$`, "m").exec(source);
  return match?.[1];
}

function readSections(source: string, name: string): ManifestSection[] {
  const sections: ManifestSection[] = [];
  const pattern = new RegExp(`^\\[\\[${escapePattern(name)}\\]\\]\\n([\\s\\S]*?)(?=^\\[\\[|(?![\\s\\S]))`, "gm");

  for (const match of source.matchAll(pattern)) {
    const body = match[1];
    if (body === undefined) {
      continue;
    }
    sections.push({
      command: readStringArray(body, "command"),
      id: readOptionalString(body, "id"),
      on: readOptionalString(body, "on")
    });
  }

  return sections;
}

function parseManifest(source: string): ManifestSnapshot {
  return {
    id: readString(source, "id"),
    name: readString(source, "name"),
    version: readString(source, "version"),
    minimumHerdrVersion: readString(source, "min_herdr_version"),
    description: readString(source, "description"),
    platforms: readStringArray(source, "platforms"),
    build: readSections(source, "build"),
    startup: readSections(source, "startup"),
    events: readSections(source, "events"),
    actions: readSections(source, "actions"),
    panes: readSections(source, "panes")
  };
}

function sameStrings(actual: string[], expected: string[]): boolean {
  return actual.length === expected.length && actual.every((value, index) => value === expected[index]);
}

function validateCommands(snapshot: ManifestSnapshot, input: ManifestValidationInput, errors: string[]): void {
  const sections = [...snapshot.build, ...snapshot.startup, ...snapshot.events, ...snapshot.actions, ...snapshot.panes];

  for (const { command } of sections) {
    if (command.length === 0) {
      errors.push("Manifest commands must not be empty.");
      continue;
    }
    if (command.some((part) => isAbsolute(part) || part.includes(".."))) {
      errors.push(`Manifest command escapes the plugin root: ${JSON.stringify(command)}`);
    }
    if (command.some((part) => FORBIDDEN_COMMAND_FRAGMENTS.some((fragment) => part.includes(fragment)))) {
      errors.push(`Manifest command uses a forbidden prototype path: ${JSON.stringify(command)}`);
    }
    if (command[0] === "node") {
      const target = command[1];
      if (target === undefined || !target.startsWith("dist/") || !target.endsWith(".js")) {
        errors.push(`Node command must target a dist JavaScript entrypoint: ${JSON.stringify(command)}`);
        continue;
      }
      const sourceTarget = `src/${target.slice("dist/".length, -".js".length)}.ts`;
      if (!input.sourceEntrypoints.includes(sourceTarget)) {
        errors.push(`Missing source entrypoint for ${target}.`);
      }
    }
  }
}

export function validateManifest(input: ManifestValidationInput): string[] {
  const errors: string[] = [];
  let snapshot: ManifestSnapshot;

  try {
    snapshot = parseManifest(input.manifestSource);
  } catch (error: unknown) {
    return [error instanceof Error ? error.message : "Manifest parsing failed."];
  }

  if (snapshot.id !== EXPECTED_ID) errors.push(`Plugin id must be ${EXPECTED_ID}.`);
  if (snapshot.name !== EXPECTED_NAME) errors.push(`Plugin name must be ${EXPECTED_NAME}.`);
  if (snapshot.description !== EXPECTED_DESCRIPTION)
    errors.push("Plugin description does not match the public identity.");
  if (snapshot.version !== input.packageVersion) errors.push("Manifest and package versions must agree.");
  if (snapshot.minimumHerdrVersion !== input.minimumHerdrVersion)
    errors.push("Minimum Herdr version does not match the contract fixture.");

  if (!sameStrings(snapshot.platforms, VERIFIED_PLATFORMS)) {
    errors.push("Manifest platforms must contain only the verified macos target.");
  }
  for (const platform of snapshot.platforms) {
    if (!input.pluginPlatforms.includes(platform)) errors.push(`Unknown Herdr plugin platform: ${platform}.`);
  }

  const events = snapshot.events.map(({ on }) => on ?? "");
  if (!sameStrings(events, REQUIRED_EVENTS))
    errors.push("Manifest lifecycle events do not match the required event set.");
  for (const event of events) {
    if (!input.eventKinds.includes(event.replaceAll(".", "_"))) errors.push(`Unknown Herdr event: ${event}.`);
  }

  if (
    !sameStrings(
      snapshot.build.map(({ command }) => command.join(" ")),
      ["npm ci", "npm run install:browser", "npm run audit:browser", "npm run build"]
    )
  ) {
    errors.push("Manifest build commands must install, audit, and build the locked renderer.");
  }
  if (snapshot.startup.length !== 1) errors.push("Manifest must define one startup entrypoint.");
  if (snapshot.actions.length !== 1 || snapshot.actions[0]?.id !== "diagnose")
    errors.push("Manifest must define the diagnose action.");
  if (snapshot.panes.length !== 1 || snapshot.panes[0]?.id !== "viewer")
    errors.push("Manifest must define the viewer pane.");

  if ([...input.manifestSource].some((character) => character.codePointAt(0)! > 0x7f)) {
    errors.push("Manifest public text must use plain English ASCII text.");
  }

  validateCommands(snapshot, input, errors);
  return errors;
}
