# Weight budget log

Targets: handoff section 4. One row per milestone measurement; never relax a target without the owner.

## M0 (2026-10-23, toolkit spikes, MacBook Air M5, on battery)

| Metric | Target | GPUI spike | iced spike |
|---|---|---|---|
| Release binary | < 40 MB | 8.5 MB ✅ | 10.2 MB ✅ |
| Idle memory: physical footprint | < 80 MB (proposed metric) | 71 MB ✅ | 120 MB ❌ |
| Idle memory: RSS (current metric) | < 80 MB | 109–121 MB ❌ | 118–126 MB ❌ |
| Cold start to first frame | < 250 ms | 148 ms ✅ | 211 ms ✅ |
| Keystroke to pixel (CPU side) | ≤ 8.3 ms | 0.36 p50 / 0.98 max ✅ | 0.16 p50 draw ✅ |
| Open 10k-line file | < 100 ms | 49 ms ✅ | 49 ms ✅ |
| RSS with 3 agents | < 150 MB | M2 | |
| Agent event to glow | < 100 ms | M3 | |

Open: the owner must approve switching the idle-memory metric from RSS to physical footprint (see `docs/spikes/toolkit.md`).
