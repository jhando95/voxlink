# UI & layout rules

The design system and the Slint layout traps behind it. Every rule here is a
postmortem — each one shipped as a visible bug at least once.

## Design system — "quiet graphite, violet accent"

- **Surfaces are preset-independent.** Graphite dark (`#0f1013` canvas →
  `#17181d` panel → `#1b1d23` card → `#232530` raised) and a mirrored light
  set. Depth is tonal plus a 1px hairline.
- **Presets are accent hues only.** Violet is the default. Nothing else may
  branch on the preset — not surfaces, radii, weights, and above all not
  *wording*. The bottom nav once renamed "HOME" to "GUIDE"/"BRIDGE" and the
  speaking chip cycled through "HOT MIC"/"COMMS"/"TX"/"On Air" with the accent
  colour, which moved both the visible label and the accessible name.
- **No gradients, glows, or drop shadows.** Drop shadows are banned
  project-wide for GPU cost. `surfaces_stay_flat_and_shadow_free` greps the
  whole UI tree and fails on `@linear-gradient` or `drop-shadow` anywhere.
- **One button language:** filled accent/danger, outline default, tonal soft,
  text ghost. No icon plates.
- **Type scale only** (22/17/14/13/12/11/10 via `VxTheme.font-*`). An off-scale
  24px title with no elide is what forced the Settings view wider than a narrow
  window.
- **No std `LineEdit`/light-chrome widgets.** `VxInput` wraps raw `TextInput`;
  build.rs pins the std style to `fluent-dark`.
- **In-call controls are fixed-size icon buttons** (`VxCallButton`) and must
  carry `accessible-label` — the icon is their only other name.

## The layout trap: Slint defaults to `stretch` alignment

This one cause has produced four distinct shipped bugs. A layout with no
`alignment` distributes spare space by growing its children; a layout **with**
any `alignment` gives children their preferred size instead.

| Symptom | Rule |
|---|---|
| Row content floating mid-row, selection bar stranded left | Never write `alignment: center` (or `start`) on a layout that has a child with `horizontal-stretch: 1` — the alignment wins and the stretch is ignored. |
| Pills/buttons stretched into full-width slabs | `horizontal-stretch: 0` on the child is **necessary but not sufficient**. A row of fixed-size controls needs `alignment: start`/`end` on the *parent*, or a spacer to absorb the slack. |
| ~60px of dead space between a card heading and its first row | Columns of stacked cards need `alignment: start`, otherwise the cards are stretched to fill the column height and their contents spread. |
| ~200px between chat messages | A content list inside a `ScrollView` with `min-height: parent.height` needs `alignment: start`, or spare viewport height is shared out between rows. |

Exception: a child with an explicit `vertical-stretch`/`horizontal-stretch` is
what *should* absorb slack, and takes precedence over `alignment` — that is how
the empty-state card keeps centring itself.

## Other Slint gotchas

- **`changed x => {}` handlers do not run without a pumping event loop**, so
  they never apply on first paint or in an offscreen render. Apply derived
  state in **`init` *and* `changed`**. This hit the theme (`dark-mode`) and the
  responsive breakpoints (`desktop-layout`/`shell-compact`).
- **Do not "fix" that by binding to `root.width`** — `desktop-layout:
  root.width >= 960px` closes a loop (width → layout info → rail presence →
  width) that Slint reports as a deprecated binding loop and may panic on.
- **No string slicing in Slint.** Avatar initials are computed in Rust; use
  `ui_shell::set_user_identity`, which sets the display name and its initial
  together. Passing a whole name to `VxAvatar.initial` clips it to the circle
  and renders the middle of the word.
- `Rectangle` has no `vertical-alignment` — use layout `alignment`.

## Narrow windows are real

`min-width` is 440px, so **every view must fit 460px**. An element that can
neither wrap nor elide reports its full width as a minimum and drags the whole
scroll content past the viewport, where it is sliced rather than elided. Give
long single-line text `overflow: elide`, and drop non-essential chips and
button labels below the `wide` breakpoint.

## How to audit

Render every screen offscreen instead of click-driving the app:

```
VOXLINK_UI_WRITE_SNAPSHOTS=1 cargo test -p ui_shell --test ui_visibility_snapshots
```

writes each scenario × narrow/wide × dark/light to
`target/ui_visibility_snapshots/` as opaque RGB PNGs. Read the frames — most of
the bugs above were invisible in the source and obvious in the render.
(`VOXLINK_START_VIEW=0..5` with `VOXLINK_DISABLE_AUTO_CONNECT=1` still launches
the real app when you need interaction.)

For anything systemic, **write a detector rather than eyeballing** — that is how
53 mis-centred rows and 14 stretched columns were found instead of the two or
three visible on one screen. Match on brace-balanced blocks and check the
indent of a property to tell a layout's own property from a child's. Beware:
single-line children (`Text { text: "…"; horizontal-stretch: 1; }`) do not match
a line-anchored property regex — missing that shipped 15 wrong edits in v0.14.2.

### Snapshot floors

Floors are per-(scenario, layout) and both match arms are exhaustive, so a new
screen must declare its own. To recalibrate after a deliberate restyle, soften
the asserts in `assert_visual_region`/`assert_snapshot_has_content` to
`eprintln!`, print every measurement, run the matrix once with `-- --nocapture`,
then set all floors ~20% under measured. Never chase one failure at a time.

**A floor calibrated against a blank region is worse than no test.**
`min_color_buckets: 2` — which a solid fill passes — is the tell. If a region
measures ≈0 deviation the rect is aimed at empty canvas: fix the rect or the
layout, do not lower the floor.
