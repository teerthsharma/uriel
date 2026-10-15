# Uriel: name and mark

**Decision (owner, 2026-10-15):** the engine is named **Uriel**. It replaces the
working names `seal-*` (crates) and `aether-mcp`. Renaming crates, binary and
repo is a Phase 2 task (PLAN.md I1) so it does not collide with Phase 1 builds
that are editing those crates.

**Why the name fits.** Uriel is the angel of light and wisdom ("the light of
God"). What the engine does is shine a small, well-chosen light on a large
graph: the model sees only the lit part, at the lowest resolution that answers.

## The mark

A flame whose silhouette holds a small graph.

| Element | Meaning |
|---|---|
| Flame | the light: the budgeted view the model reads |
| Graph inside | the system of record: nodes and edges, not a document |
| Node colour | the memory tier, and nothing else: amber is hot (the top, closest to the light), orange is warm, slate is cold |
| Halo on the apex node | the focus of the current view |
| Rim gradient | the same hot-to-cold axis as the nodes |

Colour is data here too: no node is coloured for decoration.

## Files

| File | Use |
|---|---|
| `assets/logo-mark.svg` | the mark on a dark rounded tile (app icon, avatar) |
| `assets/logo-lockup-light.svg` | mark and wordmark for light backgrounds |
| `assets/logo-lockup-dark.svg` | mark and wordmark for dark backgrounds |
| `assets/*.png` | raster exports (512 and 64 px mark, 940 px lockups, transparent) |

The wordmark is drawn as strokes, so it needs no font. Palette: amber `#FFB020`,
orange `#E8743B`, slate `#5B6B91`, ink `#0B1020`, paper `#F5F1E8`.

## Limits

At 16 px the graph inside the flame is not legible; only the flame silhouette
and the amber apex survive, so use the 64 px export or larger for icons that
must show the graph. The mark was checked by rendering in a browser at 512, 300,
64, 32 and 16 px, not by a designer.
