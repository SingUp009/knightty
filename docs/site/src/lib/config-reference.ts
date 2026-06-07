import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

import fixtureReferenceOptions from "../data/config-reference.fixture.json";
import { createUniqueSlugger } from "./slug";
import type {
  ConfigOption,
  ConfigOptionView,
  ConfigReferenceData,
  ConfigReferenceSource,
  Platform,
  ReloadBehavior,
} from "./config-reference-types";

const generatedReferencePath = resolve(process.cwd(), "../generated/config-reference.json");

const reloadBehaviors: ReloadBehavior[] = ["runtime", "new-terminal", "restart"];
const platforms: Platform[] = ["all", "windows", "linux", "macos"];

export async function getConfigReference(): Promise<ConfigReferenceData> {
  const { options, source } = await loadConfigReferenceOptions();
  const viewOptions = withViewMetadata(options);

  return {
    options: viewOptions,
    categories: uniqueValues(viewOptions.map((option) => option.category)),
    reloadBehaviors,
    platforms,
    source,
  };
}

async function loadConfigReferenceOptions(): Promise<{
  options: ConfigOption[];
  source: ConfigReferenceSource;
}> {
  const generatedOptions = await readJsonIfExists<ConfigOption[]>(generatedReferencePath);

  if (generatedOptions) {
    return { options: generatedOptions, source: "generated" };
  }

  return {
    options: fixtureReferenceOptions as ConfigOption[],
    source: "fixture",
  };
}

async function readJsonIfExists<T>(path: string): Promise<T | null> {
  try {
    return await readJson<T>(path);
  } catch (error) {
    if (isFileNotFound(error)) {
      return null;
    }

    throw error;
  }
}

async function readJson<T>(path: string): Promise<T> {
  return JSON.parse(await readFile(path, "utf8")) as T;
}

function isFileNotFound(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    (error as { code?: string }).code === "ENOENT"
  );
}

function withViewMetadata(options: ConfigOption[]): ConfigOptionView[] {
  const slugFor = createUniqueSlugger();

  return options.map((option) => ({
    ...option,
    slug: slugFor(option.key),
    searchText: buildSearchText(option),
  }));
}

function buildSearchText(option: ConfigOption): string {
  return [
    option.key,
    option.category,
    option.type,
    formatSearchValue(option.default),
    option.description,
    option.examples.join(" "),
    option.validValues?.join(" ") ?? "",
    option.reload,
    option.platform,
    option.security ?? "",
    option.since ?? "",
    option.deprecated ? "deprecated" : "",
  ]
    .join(" ")
    .toLowerCase();
}

function formatSearchValue(value: ConfigOption["default"]): string {
  return Array.isArray(value) ? value.join(" ") : String(value);
}

function uniqueValues(values: string[]): string[] {
  return [...new Set(values)];
}
