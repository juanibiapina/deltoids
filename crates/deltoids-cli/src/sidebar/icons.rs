//! Icon axis: the nerd-font glyph tables and the [`IconMode`] toggle.

/// Whether icons were requested at build time (controlled by the
/// `RV_NO_ICONS` environment variable). Captured per-`Sidebar` so tests
/// can override without touching the global env.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconMode {
    On,
    Off,
}

impl IconMode {
    /// Read `RV_NO_ICONS` from the environment. Set to anything → off.
    pub fn from_env() -> Self {
        match std::env::var_os("RV_NO_ICONS") {
            Some(v) if !v.is_empty() => IconMode::Off,
            _ => IconMode::On,
        }
    }
}

/// Pick a nerd-font glyph for the given filename.
///
/// Lookup order:
///
/// 1. Exact lowercase filename (Cargo.toml, Dockerfile, README.md,
///    package.json, go.mod, requirements.txt, etc.).
/// 2. Lowercase extension (rs, py, ts, kt, dart, elm, …).
/// 3. Generic file glyph as fallback.
///
/// Codepoints come from the nerd-fonts patched glyph set
/// (https://www.nerdfonts.com/cheat-sheet).
pub(super) fn file_icon(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if let Some(icon) = exact_filename_icon(&lower) {
        return icon;
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    extension_icon(&ext.to_ascii_lowercase())
}

/// Project-conventional filenames that get their own glyph regardless
/// of extension (e.g. `Dockerfile` has no extension; `package.json`
/// shouldn't get the generic JSON glyph).
fn exact_filename_icon(lower: &str) -> Option<&'static str> {
    const EXACT: &[(&str, &str)] = &[
        // Rust
        ("cargo.toml", "\u{e7a8}"),
        ("cargo.lock", "\u{e7a8}"),
        // Docs / licence
        ("readme", "\u{f48a}"),
        ("readme.md", "\u{f48a}"),
        ("readme.rst", "\u{f48a}"),
        ("changelog", "\u{f48a}"),
        ("changelog.md", "\u{f48a}"),
        ("contributing", "\u{f48a}"),
        ("contributing.md", "\u{f48a}"),
        ("license", "\u{f0fc3}"),
        ("license.md", "\u{f0fc3}"),
        ("licence", "\u{f0fc3}"),
        ("licence.md", "\u{f0fc3}"),
        ("copying", "\u{f0fc3}"),
        ("authors", "\u{f0fc3}"),
        // Git
        (".gitignore", "\u{f1d3}"),
        (".gitattributes", "\u{f1d3}"),
        (".gitmodules", "\u{f1d3}"),
        (".gitkeep", "\u{f1d3}"),
        (".mailmap", "\u{f1d3}"),
        // Containers / orchestration
        ("dockerfile", "\u{f308}"),
        ("containerfile", "\u{f308}"),
        (".dockerignore", "\u{f308}"),
        ("docker-compose.yml", "\u{f308}"),
        ("docker-compose.yaml", "\u{f308}"),
        ("compose.yml", "\u{f308}"),
        ("compose.yaml", "\u{f308}"),
        // Build systems
        ("makefile", "\u{e779}"),
        ("gnumakefile", "\u{e779}"),
        ("justfile", "\u{e779}"),
        ("build", "\u{e63a}"),
        ("build.bazel", "\u{e63a}"),
        ("workspace", "\u{e63a}"),
        ("workspace.bazel", "\u{e63a}"),
        ("cmakelists.txt", "\u{e615}"),
        ("meson.build", "\u{e615}"),
        // JS/TS package managers
        ("package.json", "\u{e718}"),
        ("package-lock.json", "\u{e718}"),
        ("yarn.lock", "\u{e718}"),
        ("pnpm-lock.yaml", "\u{e718}"),
        ("bun.lockb", "\u{e718}"),
        (".npmrc", "\u{e718}"),
        (".nvmrc", "\u{e718}"),
        // Python
        ("requirements.txt", "\u{e606}"),
        ("setup.py", "\u{e606}"),
        ("setup.cfg", "\u{e606}"),
        ("pyproject.toml", "\u{e606}"),
        ("poetry.lock", "\u{e606}"),
        ("pipfile", "\u{e606}"),
        ("pipfile.lock", "\u{e606}"),
        (".python-version", "\u{e606}"),
        ("tox.ini", "\u{e606}"),
        // Ruby
        ("gemfile", "\u{e739}"),
        ("gemfile.lock", "\u{e739}"),
        ("rakefile", "\u{e739}"),
        (".ruby-version", "\u{e739}"),
        // Go
        ("go.mod", "\u{e65e}"),
        ("go.sum", "\u{e65e}"),
        ("go.work", "\u{e65e}"),
        // CI / tooling configs
        ("jenkinsfile", "\u{e767}"),
        (".editorconfig", "\u{e652}"),
        (".prettierrc", "\u{e60b}"),
        (".eslintrc", "\u{e655}"),
        (".eslintrc.js", "\u{e655}"),
        (".eslintrc.json", "\u{e655}"),
        (".babelrc", "\u{e60b}"),
        (".env", "\u{f462}"),
        (".env.local", "\u{f462}"),
        (".env.development", "\u{f462}"),
        (".env.production", "\u{f462}"),
        // Shell rcfiles
        (".bashrc", "\u{f489}"),
        (".zshrc", "\u{f489}"),
        (".bash_profile", "\u{f489}"),
        (".profile", "\u{f489}"),
    ];
    EXACT.iter().find(|(k, _)| *k == lower).map(|(_, v)| *v)
}

/// Glyph for a lowercase extension (no leading dot). Falls back to a
/// generic file glyph for unknown extensions.
fn extension_icon(ext: &str) -> &'static str {
    match ext {
        // ---- Systems / native ----
        "rs" => "\u{e7a8}", // rust
        "c" => "\u{e61e}",  // c
        "h" => "\u{f0fd1}", // c header
        "cpp" | "cxx" | "cc" => "\u{e61d}",
        "hpp" | "hxx" => "\u{f0fd1}",
        "cs" => "\u{e648}", // csharp
        "fs" | "fsi" | "fsx" => "\u{e7a7}",
        "go" => "\u{e65e}",
        "java" => "\u{e738}",
        "kt" | "kts" => "\u{e634}",
        "swift" => "\u{e755}",
        "m" | "mm" => "\u{e711}", // objc
        "d" => "\u{e7af}",        // dlang
        "dart" => "\u{e798}",
        "zig" => "\u{e6a9}",
        "nim" => "\u{e677}",
        "v" => "\u{e6ac}",   // vlang
        "sol" => "\u{fcb9}", // solidity
        "hs" | "lhs" => "\u{e777}",
        "ml" | "mli" => "\u{e67a}",
        "ex" | "exs" => "\u{e62d}",
        "erl" | "hrl" => "\u{e7b1}",
        "clj" | "cljs" | "cljc" | "edn" => "\u{e768}",
        "scala" | "sc" => "\u{e737}",
        "r" | "rmd" => "\u{f25d}",
        "jl" => "\u{e624}",
        "lua" => "\u{e620}",
        "pl" | "pm" => "\u{e769}",
        "cr" => "\u{e62f}",
        "elm" => "\u{e62c}",
        "nix" => "\u{f313}",
        "vala" => "\u{e69e}",
        "vlang" => "\u{e6ac}",
        "tcl" => "\u{e6cf}",
        // ---- Web / frontend ----
        "ts" | "tsx" => "\u{e628}",
        "js" | "mjs" | "cjs" => "\u{e74e}",
        "jsx" => "\u{e7ba}",
        "vue" => "\u{fd42}",
        "svelte" => "\u{e697}",
        "astro" => "\u{e6b3}",
        "html" | "htm" | "xhtml" => "\u{f13b}",
        "css" => "\u{e749}",
        "scss" | "sass" => "\u{e74b}",
        "less" => "\u{e758}",
        "styl" | "stylus" => "\u{e600}",
        "hbs" | "handlebars" => "\u{e60f}",
        "ejs" => "\u{e618}",
        "pug" | "jade" => "\u{e60e}",
        "php" => "\u{e73d}",
        // ---- Scripts ----
        "sh" | "bash" | "zsh" | "fish" | "ksh" => "\u{f489}",
        "ps1" | "psm1" | "psd1" => "\u{ebc7}",
        "bat" | "cmd" => "\u{f17a}",
        "awk" => "\u{f489}",
        "vim" | "vimrc" => "\u{e7c5}",
        // ---- Python ----
        "py" | "pyi" | "pyc" | "pyo" => "\u{e606}",
        "ipynb" => "\u{e678}",
        // ---- Ruby ----
        "rb" | "rake" | "erb" => "\u{e739}",
        // ---- Markup / docs ----
        "md" | "markdown" | "mdx" => "\u{f48a}",
        "rst" => "\u{e6b9}",
        "tex" | "latex" | "sty" | "cls" | "bib" => "\u{e69b}",
        "adoc" | "asciidoc" => "\u{f718}",
        "org" => "\u{e633}",
        "txt" | "text" | "log" => "\u{f0219}",
        "pdf" => "\u{f1c1}",
        "epub" | "mobi" => "\u{e28b}",
        // ---- Config / data ----
        "toml" => "\u{e6b2}",
        "json" | "json5" | "jsonc" => "\u{e60b}",
        "yaml" | "yml" => "\u{f481}",
        "xml" | "xsd" | "xsl" | "xslt" => "\u{f72d}",
        "csv" | "tsv" => "\u{f1c0}",
        "sql" => "\u{e706}",
        "db" | "sqlite" | "sqlite3" => "\u{e706}",
        "parquet" | "avro" => "\u{f1c0}",
        "ini" | "cfg" | "conf" | "properties" | "plist" => "\u{e615}",
        "env" => "\u{f462}",
        // ---- Build systems ----
        "gradle" | "groovy" => "\u{e660}",
        "sbt" => "\u{e737}",
        "cmake" => "\u{e615}",
        "bazel" | "bzl" => "\u{e63a}",
        "mk" => "\u{e779}",
        // ---- Lock / archive / binary ----
        "lock" => "\u{f023}",
        "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" | "zst" => "\u{f1c6}",
        "deb" | "rpm" | "pkg" | "dmg" => "\u{f1c6}",
        "exe" | "dll" | "so" | "dylib" => "\u{f013}",
        // ---- Images / media ----
        "svg" => "\u{f81f}",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" | "tiff" => "\u{f1c5}",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => "\u{f1c7}",
        "mp4" | "mov" | "avi" | "mkv" | "webm" => "\u{f1c8}",
        // ---- Fonts ----
        "ttf" | "otf" | "woff" | "woff2" | "eot" => "\u{f031}",
        // ---- Misc ----
        "diff" | "patch" => "\u{f440}",
        "pem" | "crt" | "key" | "cer" | "pub" => "\u{f084}",
        "http" => "\u{f484}",
        _ => "\u{f15b}", // generic file
    }
}

/// Folder glyph used for directory rows. Future work could swap to a
/// closed-folder glyph when collapse/expand state lands; for now we
/// always render it as open since the tree is always fully expanded.
pub(super) const ICON_DIR_OPEN: &str = "\u{f07c}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_icon_covers_more_languages() {
        // Spot-check a handful of newly-added mappings.
        assert_eq!(file_icon("main.kt"), "\u{e634}");
        assert_eq!(file_icon("App.swift"), "\u{e755}");
        assert_eq!(file_icon("comp.vue"), "\u{fd42}");
        assert_eq!(file_icon("comp.svelte"), "\u{e697}");
        assert_eq!(file_icon("data.csv"), "\u{f1c0}");
        assert_eq!(file_icon("go.mod"), "\u{e65e}");
        assert_eq!(file_icon("requirements.txt"), "\u{e606}");
        assert_eq!(file_icon("docker-compose.yml"), "\u{f308}");
        assert_eq!(file_icon("Jenkinsfile"), "\u{e767}");
        assert_eq!(file_icon(".editorconfig"), "\u{e652}");
        assert_eq!(file_icon("flake.nix"), "\u{f313}");
        assert_eq!(file_icon("build.zig"), "\u{e6a9}");
    }

    #[test]
    fn file_icon_known_extensions() {
        assert_eq!(file_icon("lib.rs"), "\u{e7a8}");
        assert_eq!(file_icon("doc.md"), "\u{f48a}");
        assert_eq!(file_icon("Cargo.toml"), "\u{e7a8}"); // exact match wins
        assert_eq!(file_icon("config.json"), "\u{e60b}");
    }

    #[test]
    fn file_icon_unknown_extension_falls_back() {
        assert_eq!(file_icon("strange.xyz"), "\u{f15b}");
        assert_eq!(file_icon("noextension"), "\u{f15b}");
    }
}
