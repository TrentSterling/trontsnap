# Public UI captures

The current assets were captured on 2026-10-02 from the production TrontSnap UI.
The gallery uses published portfolio screenshots, never personal capture roots.

| File | Content |
|---|---|
| `gallery-dark.png` / `.webp` | Current gallery, capture menu, filters and search |
| `gallery-light.png` / `.webp` | Light gallery with a violet accent |
| `editor.png` / `.webp` | Production annotation editor with arrow, box and solid redaction |
| `theme.png` / `.webp` | Gallery with the actual theme and gradient controls |
| `og-20261002.png` | 1200 x 630 social preview composed from the gallery capture |
| `card-20261002.png` | 1200 x 630 gallery capture for the portfolio games card |

PNG links open the full-size captures. Pages load smaller WebP encodings.

Refresh with `scripts/capture-public.ps1 -PortfolioRoot <checkout>`, then
`node scripts/marketing/render-share.cjs`. Install the script's Playwright
package first. The capture uses a GPU texture, without a native window, global
input, hotkeys, tray registration or changes to persisted settings.

The product-page browser verifier also mounts `docs/` at `/trontsnap/`, so the
project's GitHub Pages source remains in this repo. The portfolio checkout only
needs its card copy and current image URL updated.
