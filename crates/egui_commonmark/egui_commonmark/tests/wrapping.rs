//! Regression tests for inline-code wrapping. The renderer used to lay out a
//! long inline-code token as a single overflowing widget, which clipped or
//! overlapped surrounding text at narrow widths. See pulldown.rs
//! `inline_code_wrap_segments`.

use egui::{Context, Rect, TextStyle};
use egui_commonmark_extended::{CommonMarkCache, CommonMarkViewer};

fn render(markdown: &str, width: f32) -> (Rect, f32) {
    let ctx = Context::default();
    let mut body_rect = Rect::NOTHING;

    // Two passes: egui caches font/layout state on the first pass.
    for _ in 0..2 {
        ctx.begin_pass(Default::default());
        egui::CentralPanel::default().show(&ctx, |ui| {
            ui.set_width(width);
            let mut cache = CommonMarkCache::default();
            let response = CommonMarkViewer::new().show(ui, &mut cache, markdown);
            body_rect = response.response.rect;
        });
        let _ = ctx.end_pass();
    }

    let body_id = TextStyle::Body.resolve(&ctx.style());
    let row_height = ctx.fonts_mut(|f| f.row_height(&body_id));
    (body_rect, row_height)
}

#[test]
fn short_inline_code_stays_on_one_row() {
    let (rect, row_height) = render("prefix `short-code` suffix", 540.0);
    assert!(
        rect.height() <= row_height * 1.5,
        "short inline code wrapped unexpectedly: rect={rect:?} row_height={row_height}"
    );
}

#[test]
fn path_like_inline_code_wraps() {
    let md = "`10-19 Infrastructure Core/10-Architecture/10-K3s-Plex-Legacy/10-Ansible-K3s-Plex/10.25-Ansible-K3s-Plex-Runbooks.md`";
    let (rect, row_height) = render(md, 540.0);
    assert!(
        rect.height() > row_height * 1.5,
        "path-like inline code did not wrap: rect={rect:?} row_height={row_height}"
    );
}

#[test]
fn unbreakable_long_inline_code_wraps() {
    let md = format!("`{}`", "A".repeat(180));
    let (rect, row_height) = render(&md, 540.0);
    assert!(
        rect.height() > row_height * 1.5,
        "unbroken long inline code did not wrap: rect={rect:?} row_height={row_height}"
    );
}

// ---------------------------------------------------------------------------
// Nested-list regression coverage (devlog/027).
//
// Pre-fix bugs:
//   1. `delayed_events_list_item` stopped at the first `TagEnd::Item`, leaking
//      outer-item events back to the outer `show()` loop when an item
//      contained a nested sub-list. The outer loop would eventually call
//      `List::start_item` with an empty stack → `unreachable!()` panic.
//   2. `show_scrollable`'s `sc.events` was parsed without the math option
//      while `show()`'s `cache.cached_events` was parsed with the math
//      option enabled at compile time. The split-point indices diverged from
//      the events Vec actually used by the viewport-skip path, so iteration
//      jumped to an unrelated event — often `Tag::Item` — and panicked the
//      same way.
//   3. Split points were added at every block-end, including ones inside
//      lists / tables / blockquotes — even with bugs 1 & 2 fixed this could
//      land iteration mid-container in the future.
//
// These tests exercise the show() and show_scrollable() paths with
// nested-list markdown. On pre-fix code each reproduced the panic.

fn nested_list_md() -> &'static str {
    "\
- outer-1 has some text
  - inner-1a
  - inner-1b
- outer-2 also has text
  - inner-2a
- outer-3 final item

Trailing paragraph with $0.02 markers and $env_var math-like content.
"
}

#[test]
fn nested_list_renders_via_show() {
    let (rect, row_height) = render(nested_list_md(), 540.0);
    assert!(
        rect.height() > row_height,
        "nested list rendered with zero height: rect={rect:?} row_height={row_height}"
    );
}

fn render_scrollable(
    markdown: &str,
    width: f32,
    height: f32,
    scroll_offset: Option<f32>,
) -> egui::Rect {
    let ctx = Context::default();
    let mut cache = CommonMarkCache::default();
    let mut inner_rect = egui::Rect::NOTHING;
    for pass in 0..3 {
        ctx.begin_pass(Default::default());
        egui::CentralPanel::default().show(&ctx, |ui| {
            ui.set_width(width);
            ui.set_height(height);
            let pending = if pass == 1 { scroll_offset } else { None };
            let out = CommonMarkViewer::new()
                .pending_scroll_offset(pending)
                .show_scrollable("scrollable_test", ui, &mut cache, markdown);
            inner_rect = out.inner_rect;
        });
        let _ = ctx.end_pass();
    }
    inner_rect
}

#[test]
fn nested_list_does_not_panic_in_show_scrollable() {
    // Three passes: bootstrap, jump via `pending_scroll_offset`, then settle.
    // Forces the viewport-clipped branch to pick a split-point landing near
    // the nested list — pre-fix this reproduced the SIGABRT seen on T470.
    let rect = render_scrollable(nested_list_md(), 540.0, 200.0, Some(80.0));
    assert!(
        rect.height() > 0.0,
        "show_scrollable produced empty content rect: {rect:?}"
    );
}

#[test]
fn deeply_nested_list_renders() {
    let md = "\
- L1
  - L2
    - L3 first
    - L3 second
  - L2 second
- L1 second
";
    let (rect, _) = render(md, 540.0);
    assert!(rect.height() > 0.0, "deeply nested list rect was empty: {rect:?}");
    let rect2 = render_scrollable(md, 540.0, 200.0, None);
    assert!(rect2.height() > 0.0, "deeply nested via scrollable empty: {rect2:?}");
}
