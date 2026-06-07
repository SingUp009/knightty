import { useMemo, useState } from "react";

import type {
  ConfigOptionView,
  ConfigReferenceSource,
  Platform,
  ReloadBehavior,
} from "../lib/config-reference-types";
import { slugify } from "../lib/slug";

type Props = {
  options: ConfigOptionView[];
  categories: string[];
  reloadBehaviors: ReloadBehavior[];
  platforms: Platform[];
  source: ConfigReferenceSource;
};

type Group = {
  category: string;
  options: ConfigOptionView[];
};

const reloadLabels: Record<ReloadBehavior, string> = {
  runtime: "Runtime",
  "new-terminal": "New terminal",
  restart: "Restart",
};

const platformLabels: Record<Platform, string> = {
  all: "All platforms",
  windows: "Windows",
  linux: "Linux",
  macos: "macOS",
};

export default function ConfigReferenceApp({
  options,
  categories,
  reloadBehaviors,
  platforms,
  source,
}: Props) {
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState("all");
  const [reload, setReload] = useState<"all" | ReloadBehavior>("all");
  const [platform, setPlatform] = useState<"all" | Platform>("all");

  const normalizedQuery = query.trim().toLowerCase();
  const filteredOptions = useMemo(
    () =>
      options.filter(
        (option) =>
          matchesQuery(option, normalizedQuery) &&
          matchesCategory(option, category) &&
          matchesReload(option, reload) &&
          matchesPlatform(option, platform),
      ),
    [category, normalizedQuery, options, platform, reload],
  );
  const groups = useMemo(
    () => groupByCategory(filteredOptions, categories),
    [categories, filteredOptions],
  );

  return (
    <div className="config-reference">
      <header className="config-reference__header">
        <div>
          <p className="config-reference__eyebrow">Configuration</p>
          <h1>Config Reference</h1>
          <p className="config-reference__lede">
            Knightty の設定項目をカテゴリ別に確認できます。現在は fixture から表示しており、 将来は
            generated JSON を優先して読み込みます。
          </p>
        </div>
        <div className="config-reference__meta" aria-label="Reference metadata">
          <span>{options.length} options</span>
          <span>source: {source}</span>
        </div>
      </header>

      <section className="config-reference__filters" aria-label="Config reference filters">
        <label className="config-reference__field config-reference__field--search">
          <span>Search</span>
          <input
            type="search"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            placeholder="font, scrollback, hyperlink..."
          />
        </label>
        <label className="config-reference__field">
          <span>Category</span>
          <select value={category} onChange={(event) => setCategory(event.currentTarget.value)}>
            <option value="all">All categories</option>
            {categories.map((value) => (
              <option value={value} key={value}>
                {value}
              </option>
            ))}
          </select>
        </label>
        <label className="config-reference__field">
          <span>Reload</span>
          <select
            value={reload}
            onChange={(event) => setReload(event.currentTarget.value as "all" | ReloadBehavior)}
          >
            <option value="all">All reload behavior</option>
            {reloadBehaviors.map((value) => (
              <option value={value} key={value}>
                {reloadLabels[value]}
              </option>
            ))}
          </select>
        </label>
        <label className="config-reference__field">
          <span>Platform</span>
          <select
            value={platform}
            onChange={(event) => setPlatform(event.currentTarget.value as "all" | Platform)}
          >
            <option value="all">All platform entries</option>
            {platforms
              .filter((value) => value !== "all")
              .map((value) => (
                <option value={value} key={value}>
                  {platformLabels[value]}
                </option>
              ))}
          </select>
        </label>
      </section>

      <div className="config-reference__body">
        <nav className="config-reference__nav" aria-label="Config categories">
          <p>Categories</p>
          {groups.map((group) => (
            <a href={`#${categorySlug(group.category)}`} key={group.category}>
              <span>{group.category}</span>
              <small>{group.options.length}</small>
            </a>
          ))}
        </nav>

        <div className="config-reference__list" aria-live="polite">
          {groups.length === 0 ? (
            <p className="config-reference__empty">No config options match the current filters.</p>
          ) : (
            groups.map((group) => (
              <section
                className="config-reference__group"
                id={categorySlug(group.category)}
                key={group.category}
              >
                <div className="config-reference__group-heading">
                  <h2>{group.category}</h2>
                  <span>{group.options.length} options</span>
                </div>
                {group.options.map((option) => (
                  <ConfigOptionCard option={option} key={option.slug} />
                ))}
              </section>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

function ConfigOptionCard({ option }: { option: ConfigOptionView }) {
  return (
    <article className="config-option" id={option.slug}>
      <div className="config-option__header">
        <div>
          <a
            className="config-option__anchor"
            href={`#${option.slug}`}
            aria-label={`Link to ${option.key}`}
          >
            #
          </a>
          <h3>{option.key}</h3>
        </div>
        <div className="config-option__badges">
          {option.deprecated && (
            <span className="config-option__badge config-option__badge--warn">Deprecated</span>
          )}
          <span className="config-option__badge">{option.type}</span>
          <span className="config-option__badge">{reloadLabels[option.reload]}</span>
          <span className="config-option__badge">{platformLabels[option.platform]}</span>
        </div>
      </div>
      <p className="config-option__description">{option.description}</p>
      <dl className="config-option__facts">
        <div>
          <dt>Default</dt>
          <dd>
            <code>{formatConfigValue(option.default)}</code>
          </dd>
        </div>
        {option.validValues && option.validValues.length > 0 && (
          <div>
            <dt>Valid values</dt>
            <dd>{option.validValues.join(", ")}</dd>
          </div>
        )}
        {option.since && (
          <div>
            <dt>Since</dt>
            <dd>{option.since}</dd>
          </div>
        )}
      </dl>
      {option.security && <p className="config-option__security">{option.security}</p>}
      {option.examples.length > 0 && (
        <div className="config-option__examples">
          <p>Examples</p>
          {option.examples.map((example) => (
            <pre key={example}>
              <code>{example}</code>
            </pre>
          ))}
        </div>
      )}
    </article>
  );
}

function matchesQuery(option: ConfigOptionView, query: string): boolean {
  return query.length === 0 || option.searchText.includes(query);
}

function matchesCategory(option: ConfigOptionView, category: string): boolean {
  return category === "all" || option.category === category;
}

function matchesReload(option: ConfigOptionView, reload: "all" | ReloadBehavior): boolean {
  return reload === "all" || option.reload === reload;
}

function matchesPlatform(option: ConfigOptionView, platform: "all" | Platform): boolean {
  return platform === "all" || option.platform === "all" || option.platform === platform;
}

function groupByCategory(options: ConfigOptionView[], categories: string[]): Group[] {
  return categories
    .map((category) => ({
      category,
      options: options.filter((option) => option.category === category),
    }))
    .filter((group) => group.options.length > 0);
}

function categorySlug(category: string): string {
  return `category-${slugify(category)}`;
}

function formatConfigValue(value: ConfigOptionView["default"]): string {
  if (Array.isArray(value)) {
    return JSON.stringify(value);
  }

  if (typeof value === "string") {
    return JSON.stringify(value);
  }

  return String(value);
}
