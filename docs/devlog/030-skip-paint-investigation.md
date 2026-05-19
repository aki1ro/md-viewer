# Investigation: Skip-Paint Virtualization Proper Fix

**Branch:** `fix/nested-list-crash`
**Date:** 2026-05-19
**Status:** Investigation / design doc; no code change in this devlog

## Why we're here

Session 029 disabled `show_scrollable`'s skip-paint code path because real-world rendering produced flicker + wrong spacing + shifted code-block indents on T470. Measured paint times after disabling (see table below) show the cost of always re-rendering the full doc each frame.

| Doc size | Events | avg paint | FPS | Verdict |
|----------|--------|-----------|-----|---------|
| Tiny (~120 lines) | 348 | 1.2 ms | 800+ | imperceptible |
| Dockerfile (~1.5k lines) | 2,514 | 5.7 ms | 175 | smooth |
| Medium synthetic | 3,453 | 11.4 ms | 88 | borderline |
| Large synthetic (~7k lines) | 20,703 | 39.0 ms | 26 | laggy |
| Huge synthetic (~36k lines) | 103,503 | 229 ms | 4 | unusable |

The "lag knee" is around 10–20k events. Below that the bootstrap path is fine; above it some form of virtualization is needed.

## Root-cause inventory: why skip-paint was broken

Three independent bugs compound:

### Bug 1 — Orphaned `Start` events
When `show_scrollable`'s slice starts at `first_event_index = split_points[above-2].event_idx`, that index points at an `End(SomeBlock)` event. The matching `Start(SomeBlock)` is outside the slice. Result:
- `End(CodeBlock)`: `CodeBlock::end()` runs with `self.code_block = None`, no-op. The code block doesn't render.
- `End(Paragraph)`: paragraph never had `Start`, so any accumulated state is missing.
- `End(Heading)`: heading text was accumulated in `current_heading_rich_texts`, but the Start that set heading-style isn't seen.

The cursor still advances past where the block *was* (via `allocate_space(0, first_end_position.y)`), so events AFTER the orphan-End render at their bootstrap-relative positions. But the block itself is missing, leaving a visual "hole" where it should have been. If that hole intersects the viewport → blank patch.

### Bug 2 — `content_size.y` inflation
The pattern `ui.set_height(page_size.y); ui.allocate_space(vec2(0, first_end_position.y));` makes egui's `ScrollAreaOutput.content_size.y` report up to **2× the real content height** when scrolled deep. Empirically: real `page_size.y = 34604`, but skip-paint reports `content_size.y = 62527` at scroll ≈30924.

This breaks two things:
- **Drift detector**: feeds the inflated value into `last_content_h - bootstrap_content_h`, triggering 619 false-positive re-bootstraps in 30 s. Partially fixed in session 029 by not updating `last_content_h` from skip-paint, but the underlying inflation is still present.
- **ScrollArea max-scroll math**: ScrollArea allows scroll up to `content_size.y - viewport.height`, so when `content_size.y = 62527`, the user can wheel-scroll to `61803` — far past real content end (33880). At those overshoot positions the viewport falls into "phantom" lower half where no events render → blank. Session 029 added a post-render clamp, but the inflation itself stays.

### Bug 3 — Container state at slice boundary
When the slice starts mid-container (list, table, blockquote, code block), the renderer's container-state stack is empty. List indent depth, blockquote depth, code-block accumulation are all wrong. Partial mitigation exists via the split-points push gate (`!self.list.is_inside_a_list() && !self.is_table && !self.is_blockquote`) but it doesn't cover all cases (code blocks aren't gated; def-lists aren't gated).

## What disabling virtualization revealed

By forcing the bootstrap path every frame, all three bugs were sidestepped because the bootstrap renders the full event stream in order — no slicing means no orphan ends, no allocate_space inflation, no missing container state. Visual rendering became flawless. Cost: O(N) per paint where N = total events.

## The proper fix: three options

### Option A — "Always include the matching Start" (slice expansion)
For each `End` event in `split_points`, record the matching `Start` event index alongside. The slice always begins at the Start, not the End.

Modify `split_points: Vec<(usize, Pos2, Pos2)>` → `Vec<(usize, usize, Pos2, Pos2)>` where the new field is `start_event_idx`.

Push site (around line 545):
```rust
// During show()'s loop, track a stack of "block-start indices" entered
// since the last split-point. When pushing the split-point for End(SomeBlock),
// the matching start is the top of the stack.
self.block_start_stack.push(event_idx);  // on Start
// ...
let matching_start = self.block_start_stack.pop().unwrap_or(event_idx);
sp.push((event_idx, matching_start, start_pos, end_pos));
```

Slicing (around line 736):
```rust
let (first_event_index, _, first_end_position) = if above >= 2 {
    let sp = &scroll_cache.split_points[above - 2];
    (sp.1, sp.2, sp.3)  // use matching_start instead of event_idx
} else { ... };
```

**Pros**: surgical, preserves the existing partition_point logic, addresses Bug 1 directly.

**Cons**: doesn't fix Bug 2 (content_size.y inflation) or Bug 3 (mid-container state). Slice expansion could grow significantly if blocks are large (e.g., a 3000-line code block in the safety margin → entire slice is one giant code block).

### Option B — Replay container open events
Before processing the slice, replay a synthetic prefix of `Start(...)` events for any container that was open at the slice boundary. The renderer state matches what it would have been mid-doc.

Requires:
- `split_points` records the container stack at that point: `Vec<(usize, Vec<ContainerOpenEvent>, Pos2, Pos2)>`
- Skip-paint pre-processes those events before iterating the slice

**Pros**: addresses Bug 3 cleanly. Combines well with Option A.

**Cons**: container state in pulldown.rs is spread across many fields (`self.list`, `self.is_table`, `self.is_blockquote`, `self.current_heading_text`, etc.). Replaying is tricky. Bug 2 remains.

### Option C — Switch to widget-level virtualization (`show_rows` pattern)
Render the markdown into a `Vec<RenderedBlock>` where each block is a self-contained widget (`Label`, `CodeBlockWidget`, `TableWidget`, etc.). On first paint render each block once and measure its height. Then use `ScrollArea::show_rows(ctx, row_height_callback, n_blocks, |ui, range| render_blocks(range))`.

**Pros**: matches egui's idiomatic pattern. Each block is self-contained, no slicing bugs possible. Both Bug 1 and Bug 3 are eliminated by construction. `show_rows` handles content sizing itself, so Bug 2 disappears too.

**Cons**: significant refactor. Markdown events aren't 1:1 with rendered blocks (a paragraph spans multiple inline events; a list spans many items). Need a "compile events → blocks" pass that groups them. Could be weeks of work.

## Recommendation

**For now: ship session 029's disabled-virtualization fix.** The 5-frame measurement confirms <3k-event docs are well within frame budget; 10–20k-event docs are borderline; only 100k-event docs are truly broken. The repo's existing target audience is "personal markdown viewer," not "render the Linux kernel docs in real-time." This is acceptable.

**Future work (separate PR, ordered by cost vs payoff):**
1. **Option A** (slice expansion) — ~1 day. Addresses Bug 1. Combined with the session-029 content_size clamp + record_header_content_y fix, this might be enough for docs up to ~50k events.
2. **Option B** (container replay) — ~2-3 days. Addresses Bug 3. Pair with Option A.
3. **Option C** (show_rows refactor) — ~1-2 weeks. The "correct" architecture but high cost.

Don't ship a partial Option A without Bug 2's content_size.y handling — the post-render clamp can break selection state if it fires every frame at scroll boundaries.

## What ships in this PR

- `record_header_content_y` (not `_if_absent`) — fixes the outline-click overshoot that was visible because first-paint header positions were captured before async font fallbacks settled.
- `show_scrollable` short-circuits to the bootstrap path every frame, with the unreachable skip-paint code preserved for future restoration.
- LESSONS.md entry summarizing the three bugs + the recommendation.
- Inline comment in `pulldown.rs` pointing here.

## Files

- `crates/egui_commonmark/egui_commonmark/src/parsers/pulldown.rs` (show_scrollable: forced bootstrap; end_tag/Heading: record_header_content_y always)
- `docs/LESSONS.md` (new entry)
- `docs/devlog/030-skip-paint-investigation.md` (this file)
