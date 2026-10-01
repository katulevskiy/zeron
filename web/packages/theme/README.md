# Browser theme sources

From the repository root:

```sh
CARGO_BUILD_JOBS=2 cargo run --locked -p zeron-theme --bin zeron-theme-export
CARGO_BUILD_JOBS=2 cargo run --locked -p zeron-theme --bin zeron-theme-export -- --check
CARGO_BUILD_JOBS=2 cargo test --locked -p zeron-theme
node web/packages/icons/scripts/generate.mjs
node web/packages/icons/scripts/generate.mjs --check
node web/packages/icons/scripts/generate-file-icons.mjs --check
node --test web/packages/icons/scripts/generation.test.mjs
```

`--check` compares both generated theme files byte-for-byte and never writes.
The exporter uses its Cargo source root, not the invocation directory. For an
isolated fixture, use `--output <directory> --browser-tokens <json>`; icon
generators similarly accept `--repo-root <fixture-root>`. Tests run the real
CLIs, verify repeat generation, change source input, and require stale checks
to fail before regeneration succeeds.

## Source authority

- `families` is the upstream `builtin_registry().families`, serialized without
  color overrides or extra fields. Theme sources, revisions, licenses and
  resolved asset hashes travel with each upstream variant.
- `accentPresets` uses upstream `AccentPreset::{color,label}`. `accents` calls
  upstream `ThemeVariant::accent_for` for every builtin variant × preset.
- `src/browser-tokens.json` owns the browser layout/glass values, motion
  catalog/springs, font manifest and three additional color roles. These
  existing PR526 values are deliberately retained, not asserted to be current
  upstream desktop tokens. Its provenance identifies the original local
  artifact; changes here require browser review and regeneration. Upstream has
  layout constants in `crates/ui/src/theme.rs`, not `zeron_proto::layout`, and
  its `proto::motion` is a loader math module, not this browser motion catalog.
  There is no automatic shared-desktop parity claim for these browser values.
- `browserColors` is separate from `ThemeVariant.colors`. The browser adapter
  maps `raisedHover`, `textDim`, and `dangerStrong` to CSS properties. Custom or
  newly added variants without an authored browser entry fall back to their
  upstream `raised`, `textMuted`, and `danger` roles, respectively. Do not copy
  Roboco's domain model or its desktop color overrides into upstream families.

Schema version 3 makes this source split explicit. Generated JSON is an output,
not a second editable token source. CI checks the theme artifact and both icon
outputs; browser typechecking/build validates their consumers.

## Artwork and fonts

Control icons come from `crates/ui/assets/icons/*.svg`. File icons and the
manifest come from `crates/ui/assets/file-icons/` and
`crates/ui/src/file-icons.json`; the existing seven desktop dark-appearance
accent lifts are applied once. File icon checks verify every light/dark output,
not just the TypeScript manifest. Source artwork must not be edited or deleted
merely to make output smaller.

Keep all source artwork notices and `fonts/licenses/THIRD_PARTY_NOTICES.md`
and `fonts/licenses/Geist-OFL.txt` with the bundled fonts. This change does not
replace, rename or remove any licensed assets or notices. The font manifest
lists existing browser font files; font CSS remains authored alongside them.
