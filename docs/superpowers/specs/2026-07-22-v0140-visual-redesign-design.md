# v0.14.0 Visual Redesign — "Quiet Graphite, Violet Accent"

**Status:** Shipped in v0.14.0.
**Trigger:** Windows testing of v0.13.x showed the shell reading as janky and
cluttered — cut-off text boxes, stray lines through text, inconsistent
buttons, native gray input chrome. The user requested a brand-new visual
design, reviewed in-app.

## Diagnosis (from live screenshots of v0.13.5)

| Symptom | Root cause |
|---|---|
| Text boxes cut off | std `LineEdit` paints platform chrome and clips descenders at some DPI/font combos; SESSION rail card content wider than the rail (`clip: true` truncated text/pills mid-word) |
| Random lines through text | Multi-hue gradient hairline under the TopBar crossing the breadcrumb chips; status pills stretched full-row by Slint's default layout stretch, their 1px borders reading as lines; per-preset scanline/star decorations in `VxCard` |
| Janky/cluttered | Three coexisting button styles; status shown in 4 places at once; seven full-tint theme presets multiplying inconsistency; heavy navy panels with mixed radii |

## The system

- **Surfaces (preset-independent):** graphite dark (`#0f1013` canvas →
  `#17181d` panel → `#1b1d23` card → `#232530` raised) and mirrored light.
  Depth is tonal + 1px hairline (`#262832` / `#e2e3e8`). No gradients, no
  glows, no shadows (project rule).
- **Presets = accent hues only:** Voxlink violet `#8b76ff`/`#6544e0` default;
  green/blue/mint/amber/steel/cyan variants. All former per-preset surface,
  border, radius, weight, and uppercase branching collapsed to constants.
- **Type scale:** unchanged tokens (22/17/14/13/12/11/10) with weights
  600/500/400; overlines are 11px/600/+0.8px muted.
- **Components:** one button language (filled accent/danger, outline default,
  tonal soft, ghost) with no icon plates and `horizontal-stretch: 0`;
  `VxInput` rebuilt on raw `TextInput` (custom placeholder, focus ring,
  click-anywhere focus, `forward-focus`); flat `VxCard`; flat 22px chip
  `VxStatusPill` (no dot, no border, no stretch); single-hairline `AccentBar`;
  one `VxLogo` mark (accent tile + three white bars) replacing 377 lines of
  per-preset logo art.
- **Shell:** 52px flat TopBar (title + optional subtitle, minimal chips, one
  connection chip) over a hairline; breadcrumb strip removed (TopBar title +
  rail selection carry context; Esc/back buttons unchanged); rail = brand row,
  optional live-room shortcut row, 34px `RailItem` nav, spaces list, flat user
  footer — SESSION card deleted; reconnect banner demoted to a quiet warning
  strip shown only outside Home.
- **Std-widget chrome:** build.rs pins the Slint style to `fluent-dark` so the
  remaining std widgets (multi-line composer `TextEdit`, scrollbars) follow
  the theme on every platform.

## Testing approach

Source-shape tests (`ui_visibility_layout.rs`) updated to lock the new
invariants (TextInput-based field, 44/36px input heights, hugging action
buttons). Snapshot tests (`ui_visibility_snapshots.rs`) recalibrated from
harvested measurements: luma-deviation and edge floors set ~15–20% below
observed healthy values — blank or garbled renders (≈0–2 deviation, ≈0 edges)
still fail every check.

## Explicitly out of scope

Light-mode polish pass beyond the mirrored palette, room/system view deep
restructuring (inherited the system via tokens/components), and any
protocol/server change (release is client-only).

## Follow-up — v0.14.1

The deferred items above were completed in v0.14.1, which took the room (live
call) view through the same treatment: fixed-size circular call controls
replacing full-width text slabs, flat header and stage, tiles sized to the
stage, one preset-independent speaking chip, and a quieter status strip. The
last pre-v0.14 leftovers elsewhere went with it — four full-window gradient
washes behind the whole shell, the slider drop shadow, the avatar sheen,
decorative cap strips, and the theme-preset card gradients.

Auditing the room view surfaced five rendering bugs (see CHANGELOG v0.14.1),
which prompted the more important finding: the snapshot matrix was rendering
the same configuration four times. Light mode never rendered, and narrow never
rendered narrow, so the "light" and "narrow" halves were duplicates — and two
region assertions had been calibrated down onto blank canvas as a result. The
room view, the app's primary screen, was not in the matrix at all. Both gaps
are closed; floors are now per-layout, exhaustive, and set from harvested
measurements of genuinely light and genuinely narrow renders.
