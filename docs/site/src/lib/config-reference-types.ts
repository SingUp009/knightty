export type ReloadBehavior = "runtime" | "new-terminal" | "restart";

export type Platform = "all" | "windows" | "linux" | "macos";

export type ConfigValue = string | number | boolean | string[] | null;

export type ConfigOption = {
  key: string;
  category: string;
  type: string;
  default: ConfigValue;
  description: string;
  examples: string[];
  validValues?: string[];
  range?: string;
  reload: ReloadBehavior;
  platform: Platform;
  security?: string;
  since?: string;
  deprecated?: boolean;
};

export type ConfigOptionView = ConfigOption & {
  slug: string;
  searchText: string;
};

export type ConfigReferenceSource = "generated" | "fixture";

export type ConfigReferenceData = {
  options: ConfigOptionView[];
  categories: string[];
  reloadBehaviors: ReloadBehavior[];
  platforms: Platform[];
  source: ConfigReferenceSource;
};
