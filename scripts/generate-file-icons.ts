import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const usage = "Usage: bun scripts/generate-file-icons.ts <t3code checkout>";

const source = process.argv[2];
if (!source || source === "--help" || source === "-h") {
  console.error(usage);
  process.exit(source ? 0 : 2);
}

const repo = resolve(import.meta.dir, "..");
const t3code = resolve(source);
const pnpm = join(t3code, "node_modules", ".pnpm");
const treesPackage = readdirSync(pnpm).find((entry) => entry.startsWith("@pierre+trees@1.0.0-beta.6"));
if (!treesPackage) {
  console.error(`no @pierre/trees 1.0.0-beta.6 under ${pnpm}`);
  process.exit(1);
}
const trees = join(pnpm, treesPackage, "node_modules", "@pierre", "trees");
const builtIn = readFileSync(join(trees, "dist", "builtInIcons.js"), "utf8");
const style = readFileSync(join(trees, "dist", "style.js"), "utf8");
const t3Icons = readFileSync(join(t3code, "apps", "web", "src", "pierre-icons.ts"), "utf8");
const t3Video = readFileSync(join(t3code, "packages", "shared", "src", "video.ts"), "utf8");
const paneflowSource = join(repo, "scripts", "file-icons", "paneflow");
const outRoot = join(repo, "src-app", "assets", "icons", "files");
const tablesOut = join(repo, "src-app", "src", "file_icons", "tables.rs");

type Variant = "light" | "dark";
const variants: Variant[] = ["light", "dark"];

const palette = new Map<string, Record<Variant, string>>();
for (const match of style.matchAll(/--trees-icon-([a-z]+): light-dark\((#[0-9a-fA-F]+), (#[0-9a-fA-F]+)\)/g)) {
  palette.set(match[1], { light: match[2], dark: match[3] });
}
const tokenColor = new Map<string, string>();
for (const match of style.matchAll(/--trees-file-icon-color-([a-z0-9-]+): var\(--trees-file-[a-z-]+, var\(--trees-icon-([a-z]+)\)\)/g)) {
  tokenColor.set(match[1], match[2]);
}
const gray = palette.get("gray");
if (!gray) {
  console.error("the @pierre/trees palette has no gray entry");
  process.exit(1);
}

function colorFor(token: string, variant: Variant): string {
  const name = tokenColor.get(token);
  return (name ? palette.get(name) : undefined)?.[variant] ?? gray![variant];
}

const documentColors = new Map<string, string>();
for (const match of t3Icons.matchAll(/"(t3-file-icon-[a-z-]+)": "(light-dark\([^"]+\))"/g)) {
  documentColors.set(match[1], match[2]);
}

function resolveColors(body: string, fallback: string, variant: Variant): string {
  return body
    .replace(/\$\{DOCUMENT_ICON_COLORS\["([a-z0-9-]+)"\]\}/g, (_, key: string) => documentColors.get(key) ?? fallback)
    .replace(/var\(--[a-z-]+,\s*(light-dark\([^)]*\))\)/g, "$1")
    .replace(/light-dark\(\s*(#[0-9a-fA-F]+)\s*,\s*(#[0-9a-fA-F]+)\s*\)/g, (_, light: string, dark: string) =>
      variant === "light" ? light : dark,
    )
    .replace(/currentColor/gi, fallback);
}

function svgDocument(viewBox: string, body: string, fill: string): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="${viewBox}" fill="${fill}">\n${body.trim()}\n</svg>\n`;
}

interface Icon {
  token: string;
  render: (variant: Variant) => string;
}

const icons = new Map<string, Icon>();

for (const match of builtIn.matchAll(/<symbol id="file-tree-builtin-([a-z0-9-]+)" viewBox="([^"]+)">([\s\S]*?)<\/symbol>/g)) {
  const [, token, viewBox, body] = match;
  icons.set(token, {
    token,
    render: (variant) => {
      const color = colorFor(token, variant);
      return svgDocument(viewBox, resolveColors(body, color, variant), color);
    },
  });
}
const builtInCount = icons.size;

for (const match of t3Icons.matchAll(/<symbol id="t3-file-icon-([a-z0-9-]+)" viewBox="([^"]+)">([\s\S]*?)<\/symbol>/g)) {
  const [, token, viewBox, body] = match;
  if (icons.has(token)) {
    console.error(`t3code icon ${token} collides with a @pierre/trees icon`);
    process.exit(1);
  }
  icons.set(token, {
    token,
    render: (variant) => svgDocument(viewBox, resolveColors(body, gray![variant], variant), gray![variant]),
  });
}
const t3Count = icons.size - builtInCount;

for (const file of readdirSync(paneflowSource).filter((name) => name.endsWith(".svg")).sort()) {
  const token = file.slice(0, -4);
  if (icons.has(token)) {
    console.error(`Paneflow icon ${token} collides with an upstream icon`);
    process.exit(1);
  }
  const text = readFileSync(join(paneflowSource, file), "utf8");
  const sized = text.replace(/<svg\b([^>]*)>/, (_, attributes: string) => {
    const kept = attributes.replace(/\s(width|height)="[^"]*"/g, "");
    return `<svg width="16" height="16"${kept}>`;
  });
  icons.set(token, { token, render: () => sized });
}

function objectEntries(name: string, text: string): [string, string][] {
  const start = text.indexOf(`const ${name} = {`);
  if (start < 0) {
    console.error(`no ${name} table`);
    process.exit(1);
  }
  const end = text.indexOf("};", start);
  const block = text.slice(start, end);
  return [...block.matchAll(/^\s*(?:"([^"]+)"|([A-Za-z0-9_]+)):\s*"([^"]+)"/gm)].map((match) => [
    (match[1] ?? match[2]).toLowerCase(),
    match[3],
  ]);
}

function t3Entries(key: "byFileName" | "byFileExtension"): [string, string][] {
  const start = t3Icons.indexOf(`${key}: {`);
  const end = t3Icons.indexOf("\n  },", start);
  const block = t3Icons.slice(start, end);
  const entries: [string, string][] = [];
  for (const match of block.matchAll(/^\s*(?:"([^"]+)"|([A-Za-z0-9_]+)):\s*"([^"]+)"/gm)) {
    entries.push([(match[1] ?? match[2]).toLowerCase(), t3Token(match[3])]);
  }
  const video = block.match(/VIDEO_FILE_EXTENSIONS\.map\(\(extension\) => \[extension, "([^"]+)"\]\)/);
  if (video) {
    for (const extension of t3Video.matchAll(/\["([a-z0-9]+)", "video\/[^"]+"\]/g)) {
      entries.push([extension[1], t3Token(video[1])]);
    }
  }
  return entries;
}

function t3Token(name: string): string {
  return name.replace(/^file-tree-builtin-/, "").replace(/^t3-file-icon-/, "");
}

const tables: Record<string, [string, string][]> = {
  BUILT_IN_NAMES: objectEntries("BUILT_IN_FILE_NAME_TOKENS", builtIn),
  BUILT_IN_EXTENSIONS: objectEntries("BUILT_IN_FILE_EXTENSION_TOKENS", builtIn),
  COMPLETE_EXTENSION_OVERRIDES: objectEntries("COMPLETE_EXTENSION_OVERRIDES", builtIn),
  T3CODE_NAMES: t3Entries("byFileName"),
  T3CODE_EXTENSIONS: t3Entries("byFileExtension"),
};

for (const [table, entries] of Object.entries(tables)) {
  for (const [key, token] of entries) {
    if (!icons.has(token)) {
      console.error(`${table} maps ${key} to ${token}, which has no icon`);
      process.exit(1);
    }
  }
}

for (const variant of variants) {
  const dir = join(outRoot, variant);
  if (existsSync(dir)) rmSync(dir, { recursive: true });
  mkdirSync(dir, { recursive: true });
  for (const icon of icons.values()) {
    writeFileSync(join(dir, `${icon.token}.svg`), icon.render(variant));
  }
}

function rustTable(name: string, entries: [string, string][]): string {
  const unique = new Map<string, string>();
  for (const [key, token] of entries) unique.set(key, token);
  const sorted = [...unique.entries()].sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0));
  const rows = sorted.map(([key, token]) => `    (${JSON.stringify(key)}, ${JSON.stringify(token)}),`).join("\n");
  return `pub(super) const ${name}: &[(&str, &str)] = &[\n${rows}\n];\n`;
}

const tokens = [...icons.keys()].sort();
const rust = [
  ...Object.entries(tables).map(([name, entries]) => rustTable(name, entries)),
  `pub(super) const TOKENS: &[&str] = &[\n${tokens.map((token) => `    ${JSON.stringify(token)},`).join("\n")}\n];\n`,
].join("\n");
mkdirSync(join(repo, "src-app", "src", "file_icons"), { recursive: true });
writeFileSync(tablesOut, rust);
const formatted = spawnSync("rustfmt", ["--edition", "2024", tablesOut], { stdio: "inherit" });
if (formatted.status !== 0) {
  console.error("rustfmt failed on the generated tables");
  process.exit(1);
}

console.log(
  `wrote ${icons.size} icons per variant (${builtInCount} from @pierre/trees, ${t3Count} from t3code, ${icons.size - builtInCount - t3Count} from Paneflow) and ${tablesOut}`,
);
