# Fix: outline-click scroll target landing at wrong position

**Status:** ✅ Complete
**Branch:** `fix/nested-list-crash`
**Date:** 2026-05-17
**Lines Changed:** part of the same commit as devlog 027's crash fixes; outline-scroll portion is ~30 lines in `pulldown.rs` + `misc.rs` + doc updates

## Summary

Clicking outline headers scrolled to the wrong content — for far-from-current-scroll targets, the visible viewport landed hundreds of pixels off from the clicked heading. Two independent bugs combined to produce the symptom:

1. **Header position cache drift**: `record_header_content_y` overwrote header positions every paint, and subsequent paints recorded ~24 px off from the bootstrap value (font/image loading completes between frames, shifting layout slightly).
2. **`split_points` coord-system mismatch**: `ScrollableCache::split_points` stored `ui.next_widget_position()` directly — which is screen-y — but the consumers (`partition_point` and `allocate_space`) interpret those values as content-y. The mismatch is small at scroll=0 (~44 px panel-chrome offset) but blows up to hundreds of pixels at non-zero scroll, which is exactly the scenario the outline-click forced-bootstrap triggers.

## Features

- [x] Cache stability: header positions locked to first sighting, invalidated on content reload
- [x] Coord-system fix: split_points stored in content-y, matching consumer expectations
- [x] Verified end-to-end via egui MCP on a 7000-line doc with outline clicks landing correctly

## Key Discoveries

### Discovery 1 — `record_header_content_y` was overwriting and drifting

`CommonMarkCache::record_header_content_y` was called from `end_tag(Heading)` on every paint. Even at the same nominal scroll position, two consecutive paints recorded different content_y values for the same heading:

| Heading | First (bootstrap) paint at scroll=0 | Subsequent paint at scroll=0 |
|---------|-------------------------------------|------------------------------|
| `Recent Changes Log` | content_y=279 (cursor=323, min_rect_top=44) | content_y=303 (cursor=347, min_rect_top=44) |
| `2026-05-17` | 442.5 | 466.5 |

The 24-px drift correlates with font/image loading completing between frames. Whichever paint ran last before the user clicked the outline header won the cache slot — meaning the click target was effectively non-deterministic.

**Fix:** add `record_header_content_y_if_absent` and route the renderer through it instead of the overwriting variant. First-sighting wins. Cache cleared on content_version change (file reload) inside `show_scrollable`'s invalidation block.

```rust
// In CommonMarkCache (misc.rs)
pub fn record_header_content_y_if_absent(&mut self, key: &str, content_y: f32) {
    self.header_positions.entry(key.to_string()).or_insert(content_y);
}
```

### Discovery 2 — `split_points` stored screen-y, but consumers expected content-y (root cause)

This was the bigger bug. The empirical signal:

| Path | Scroll | Title screen_y | Title content_y per formula | Visible? |
|------|--------|----------------|------------------------------|----------|
| First bootstrap | 0 | 323 | 279 | ✓ mid-viewport |
| Wheel-scroll to 230 | 230 | 117 | 303 | ✓ visible |
| Forced bootstrap after outline-click | 229 | 118 | 303 | ✓ (this frame only) |
| **Subsequent skip-paint at same scroll** | 229 | **-67** | **118** | **off-screen above** |

Same `state.offset.y=229`, but the rendering produced by the wheel-scroll path put the title at screen_y=117 (visible) while the outline-click path put it at screen_y=-67 (off-screen). Two completely different layouts at the same nominal scroll.

The reason: `pulldown.rs:486` and `:528` stored `ui.next_widget_position()` directly. That returns **screen-y** in the cursor's coord system, which equals `min_rect.top() + content_y`. When `min_rect.top() = 44` (scroll=0) the difference is small; when `min_rect.top() = -185` (scroll=229) the stored y values diverge from content-y by ~229.

The skip-paint's `partition_point(|(_, _, end)| end.y < viewport.min.y)` then compares screen-y (stored) against content-y (`viewport.min.y`). At scroll=229 it picks a split-point ~229 px earlier than it should, and `allocate_space(first_end_position.to_vec2())` advances the cursor by screen-y instead of content-y, landing rendering at the wrong content position.

**Fix:**

```rust
// In show()'s event loop (pulldown.rs:486 + :528)
let min_top = ui.min_rect().top();
let raw_start = ui.next_widget_position();
let start_position = egui::pos2(raw_start.x, raw_start.y - min_top);
// ... process_event ...
let raw_end = ui.next_widget_position();
let end_position = egui::pos2(raw_end.x, raw_end.y - min_top);
```

All three consumers (`pulldown.rs:701` and `:713` partition_points, `:737` `allocate_space`) naturally interpret content-y correctly — no consumer changes needed.

**Bonus side-effect:** at scroll=0 the previous code was off by ~44 px (panel-chrome). After the fix this drift is gone, so outline-click precision improves at scroll=0 too.

### Discovery 3 — visual debugging needed in-renderer instrumentation, not just state logs

Initial debugging logged `scroll_output.state.offset.y` and `pending_scroll_offset`. These showed scroll going where requested (`0 → 229`), and the cache value being read correctly (`279`, then `303` after drift was fixed). But the *visible rendering* didn't match. Without in-renderer instrumentation logging `ui.cursor().top()`, `ui.min_rect().top()`, and the computed content_y per heading per paint, the coord-system mismatch was invisible — the state was internally consistent within each individual paint, but the comparison was between **different** paints' coord systems.

The diagnostic that finally cracked it: log every heading's `screen_y` + `min_rect_top` during EVERY paint, then compare values for the same heading across two paints at the same nominal scroll position. The discrepancy was the bug.

## Architecture

### New Function

| Function | File | Purpose |
|----------|------|---------|
| `CommonMarkCache::record_header_content_y_if_absent` | `egui_commonmark_backend/src/misc.rs` | First-sighting-wins variant of `record_header_content_y`. Prevents subsequent-paint drift from corrupting the cached header position. |

### Modified Behavior

- `pulldown.rs` `show()` event loop: split_points are now stored with `content_y = screen_y - min_rect.top()` instead of raw screen-y.
- `pulldown.rs` `show_scrollable()` content_version invalidation: now also calls `cache.clear_header_positions()` so file reload resets the header position cache cleanly.

## Testing Notes

End-to-end verification on `Recent-Changes.md` (~7000 lines, ~30k events) via egui MCP at `DISPLAY=:99`:

| Test | Before fix | After fix |
|------|-----------|-----------|
| Click outline `Recent Changes Log` (title) | Title off-screen above; `2026-05-17` h2 at top | Title visible at viewport top with frontmatter context above |
| Click outline `2026-05-17` (early h2) | Section content visible, heading off-screen | Heading at viewport top |
| Click far-deep h3 | Lands ~viewport-height off from target | Lands close to target (~50-100 px below ideal — see Future Improvements) |
| Wheel-scroll then back to outline | Inconsistent scroll behavior | Consistent across wheel and outline paths |

Unit tests added during the related crash-fix work (`tests/wrapping.rs::nested_list_does_not_panic_in_show_scrollable`, etc.) all still pass — no regressions.

## Future Improvements

- [ ] Deep-heading precision: clicking a heading 17k+ px down lands just before the target instead of at the viewport top. Likely tuning the `above >= 2` safety margin to `above >= 1` would help, but needs testing against inline-flow content clipping (see `LESSONS.md` "Slice-clone, not Vec-clone-then-skip" entry for the safety-margin rationale).
- [ ] Cache-key inline-code mismatch: headings containing backticks (e.g. `### feat — Bundle 13 (commit ` `` `c93c45c` `` `)`) hit cache MISS at click time because the app's `normalized_title` keeps backticks while the renderer's `current_heading_text` strips them. Falls through to line-ratio fallback. Small fix in `parse_headers()` to align normalization.
- [ ] Address layout-stability proper: the 24-px drift between bootstrap and subsequent paints (Discovery 1) is itself a smell. Locking to first-sighting values is a workaround, not a root-cause fix — the renderer SHOULD produce stable layouts across paints. Investigate font/image loading hooks to detect "layout-stable" frames and prefer recordings from those.

## See also

- `docs/devlog/027-nested-list-crash-fix.md` — sibling work that shipped in the same commit; covers the nested-list SIGABRT + slice-clone perf fix.
- `docs/LESSONS.md` entries: "split_points must store content-y, not screen-y", "Outline scroll-to: virtualization breaks the corrective y-record loop" (older note that pre-figured this work).
