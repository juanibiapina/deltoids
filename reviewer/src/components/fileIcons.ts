// Per-file-type icons, using tree-shaken brand logos from `simple-icons`
// (only the languages the reviewer actually shows are imported, so the bundle
// stays small). Maps a filename to a monochrome SVG path + a display colour,
// mirroring the intent of the TUI's per-extension icon table.

import {
  siTypescript,
  siJavascript,
  siReact,
  siRust,
  siPython,
  siGo,
  siRuby,
  siC,
  siCplusplus,
  siCss,
  siSass,
  siHtml5,
  siJson,
  siMarkdown,
  siYaml,
  siToml,
  siGnubash,
  siAstro,
  siVuedotjs,
  siSvelte,
  siPhp,
  siLua,
  siSwift,
  siKotlin,
  siDocker,
  siNpm,
} from "simple-icons";

export interface FileIcon {
  path: string;
  color: string;
}

interface SimpleIcon {
  path: string;
  hex: string;
}

// Some brand colours are near-black (Rust, JSON, Markdown, Lua) and vanish on
// the dark sidebar, so override those with a readable colour.
function icon(si: SimpleIcon, override?: string): FileIcon {
  return { path: si.path, color: override ?? `#${si.hex}` };
}

// Exact basename matches (checked before the extension map).
const BY_NAME: Record<string, FileIcon> = {
  "package.json": icon(siNpm),
  "package-lock.json": icon(siNpm),
  "Cargo.toml": icon(siRust, "#FF7043"),
  "Cargo.lock": icon(siRust, "#FF7043"),
  Dockerfile: icon(siDocker),
  "tsconfig.json": icon(siTypescript),
};

// Extension matches (lowercased, without the dot).
const BY_EXT: Record<string, FileIcon> = {
  ts: icon(siTypescript),
  mts: icon(siTypescript),
  cts: icon(siTypescript),
  tsx: icon(siReact),
  js: icon(siJavascript),
  mjs: icon(siJavascript),
  cjs: icon(siJavascript),
  jsx: icon(siReact),
  rs: icon(siRust, "#FF7043"),
  py: icon(siPython),
  go: icon(siGo),
  rb: icon(siRuby),
  c: icon(siC),
  h: icon(siC),
  cc: icon(siCplusplus),
  cpp: icon(siCplusplus),
  cxx: icon(siCplusplus),
  hpp: icon(siCplusplus),
  hh: icon(siCplusplus),
  css: icon(siCss, "#8A6DF1"),
  scss: icon(siSass),
  sass: icon(siSass),
  html: icon(siHtml5),
  htm: icon(siHtml5),
  json: icon(siJson, "#F5A623"),
  json5: icon(siJson, "#F5A623"),
  jsonc: icon(siJson, "#F5A623"),
  md: icon(siMarkdown, "#67B7F7"),
  markdown: icon(siMarkdown, "#67B7F7"),
  mdx: icon(siMarkdown, "#67B7F7"),
  yml: icon(siYaml),
  yaml: icon(siYaml),
  toml: icon(siToml, "#C4926B"),
  sh: icon(siGnubash),
  bash: icon(siGnubash),
  zsh: icon(siGnubash),
  astro: icon(siAstro),
  vue: icon(siVuedotjs),
  svelte: icon(siSvelte),
  php: icon(siPhp),
  lua: icon(siLua, "#6A8DFF"),
  swift: icon(siSwift),
  kt: icon(siKotlin),
  kts: icon(siKotlin),
};

// Icon for a filename, or null to fall back to the generic file glyph.
export function fileIcon(name: string): FileIcon | null {
  const base = name.slice(name.lastIndexOf("/") + 1);
  if (BY_NAME[base]) return BY_NAME[base];
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return null;
  const ext = base.slice(dot + 1).toLowerCase();
  return BY_EXT[ext] ?? null;
}
