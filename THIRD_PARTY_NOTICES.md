# TableScale third-party notices

TableScale is MIT licensed. Its Rust dependencies are permissively licensed;
most are MIT and/or Apache-2.0. The direct dependencies at release time are:

- `eframe` / `egui` — MIT OR Apache-2.0
- `ab_glyph` — Apache-2.0
- `epaint_default_fonts` — (MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0
- `glam` — MIT OR Apache-2.0
- `i_overlay` — MIT OR Apache-2.0
- `image` — MIT OR Apache-2.0
- `png` — MIT OR Apache-2.0
- `printpdf` — MIT
- `rfd` — MIT
- `spade` — MIT OR Apache-2.0
- `accesskit` (via eframe) — MIT OR Apache-2.0

The 3D base nameplate uses the **Ubuntu Light** font embedded through
`epaint_default_fonts`. The font is distributed under the Ubuntu Font Licence
1.0 and is used only to generate nameplate outlines at runtime; no system font
is loaded. See the upstream `epaint_default_fonts` crate for the bundled font
licence texts.

The complete, exact dependency set is recorded in `Cargo.lock`. License texts
for transitive dependencies are available from their respective crates and
upstream repositories. If you redistribute TableScale, include this notice and
the `LICENSE` file alongside the binary.
