# Vendored syntax themes

## tokyonight.tmTheme

Tokyo Night (Night variant) TextMate theme, vendored so the syntax-theme
registry can offer it identically on native and wasm.

- Source: [`folke/tokyonight.nvim`](https://github.com/folke/tokyonight.nvim),
  file `extras/sublime/tokyonight_night.tmTheme`.
- Author: Folke Lemaitre.
- License: Apache License 2.0.

The upstream project is licensed under Apache-2.0. This file is redistributed
under those terms; the license text is preserved with the project and this
attribution satisfies the notice requirement. The deltoids project itself is
MIT-licensed; bundling a permissively licensed asset alongside it is
compatible.

At build time `build.rs` converts this plist into a compressed syntect dump
(`$OUT_DIR/tokyonight.themedump`) so no plist parser is needed at runtime on
either target.
