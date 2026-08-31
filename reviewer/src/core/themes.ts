// Curated syntax-theme names for the reviewer's picker, grouped by the chrome
// mode they read best against. Every name must exist in the wasm registry
// (two-face's embedded themes plus the vendored Tokyo Night); an unknown name
// would harmlessly fall back to the engine default, but the list is kept in
// sync on purpose. See `deltoids::theme_names`.

export const SYNTAX_THEMES = {
  dark: [
    "TokyoNight",
    "Monokai Extended",
    "Dracula",
    "Nord",
    "OneHalfDark",
    "TwoDark",
    "Solarized (dark)",
    "gruvbox-dark",
    "Coldark-Dark",
    "Visual Studio Dark+",
    "zenburn",
  ],
  light: [
    "GitHub",
    "InspiredGitHub",
    "OneHalfLight",
    "Solarized (light)",
    "gruvbox-light",
    "Coldark-Cold",
    "Monokai Extended Light",
  ],
} as const;

// Defaults derived from the chrome mode when the user has not chosen a syntax
// theme. Matches the native mode defaults in spirit (dark → Tokyo Night,
// light → GitHub).
export const DEFAULT_DARK_SYNTAX_THEME = "TokyoNight";
export const DEFAULT_LIGHT_SYNTAX_THEME = "GitHub";
