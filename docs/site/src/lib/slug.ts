export function slugify(value: string): string {
  const slug = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");

  return slug || "item";
}

export function createUniqueSlugger() {
  const seen = new Map<string, number>();

  return (value: string): string => {
    const baseSlug = slugify(value);
    const count = seen.get(baseSlug) ?? 0;
    seen.set(baseSlug, count + 1);

    return count === 0 ? baseSlug : `${baseSlug}-${count + 1}`;
  };
}
