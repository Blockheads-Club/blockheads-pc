# Fonts of the in-game mod UI

- `AtkinsonHyperlegibleNext-Regular.ttf` (body text of the join dialog and mod panel): Atkinson Hyperlegible Next,
  Copyright 2020-2024 The Atkinson Hyperlegible Next Project Authors. SIL Open Font License 1.1,
  text in `AtkinsonHyperlegibleNext-OFL.txt`. Unmodified.
- `crates/bhpc-mods/fonts/Tiny5-Regular.ttf` (titles of the mod panel): Tiny5, SIL Open Font License 1.1,
  text in `Tiny5-OFL.txt` (a copy of `crates/bhpc-mods/fonts/Tiny5-OFL.txt`). Unmodified.
- Bold text and glyphs Atkinson lacks (Cyrillic, ...) use Liberation Sans (`crates/bhpc-vfs/fonts`, OFL 1.1).

The fonts are embedded in the `bhpc-mods` library with `include_bytes!`. Selling the fonts by themselves is not
allowed by the OFL; they are shipped as part of this program.
