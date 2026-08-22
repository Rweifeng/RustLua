# Third-Party Assets & Licenses

Assets collected for the M6-06 sample game (top-down roguelite demo). All are
CC0 1.0 Universal (public domain) unless noted — free for personal and
commercial use, no attribution required (appreciated).

| Asset | Source | License | Used as |
|---|---|---|---|
| Micro Roguelike tiles (colored) | [Kenney.nl – Micro Roguelike](https://kenney.nl/assets/micro-roguelike) | CC0 | `game/assets/textures/tileset.png` (wall/floor assembled from `tile_0000/0002/0005/0012/0016/0020`) |
| Impact Sounds (`impactMetal_medium_000`, `impactPunch_medium_000`, `impactGlass_medium_000`) | [Kenney.nl – Impact Sounds](https://kenney.nl/assets/impact-sounds) | CC0 | `game/assets/audio/{attack,hit,pickup}.ogg` |
| (Character/enemy/coin sprites) | Procedurally generated in `scripts/gen-characters.ps1` | engine's own | `game/assets/textures/hero-*.png`, `slime.png`, `coin.png` |
| (Earlier placeholder strips) | Generated in `scripts/gen-textures.ps1` | engine's own | `game/assets/textures/*-sheet.png`, `player.png` (superseded) |
| M8 test triangle GLB | Generated in `scripts/gen-m8-test-gltf.ps1` | Self-authored, no third-party content | `game/assets/models/m8-test-triangle.glb` |

Downloaded packs (kept for regeneration only, not committed):
- `kenney_micro-roguelike.zip`, `kenney_impact-sounds.zip`, `kenney_roguelike-characters.zip`

The engine and game source code are MIT OR Apache-2.0.

The M8 test triangle GLB is authored by this project and contains only generated
geometry data. It has no third-party dependency or attribution requirement. The
file can be regenerated with `scripts/gen-m8-test-gltf.ps1`.
