# Local patches to winit 0.30.13 (Tabular)

`Cargo.toml` of the workspace uses `[patch.crates-io] winit = { path = "patches/winit" }`.
This directory is a verbatim copy of `winit-0.30.13` from crates.io except for the files
listed here. When bumping winit, re-apply each item (or drop it if upstream has caught up).
Re-generate this list with `diff -rq src ~/.cargo/registry/src/*/winit-<ver>/src`.

## 1. `src/platform_impl/macos/ffi.rs` — remove private CGS blur FFI

Upstream declares the private `CGSMainConnectionID` / `CGSSetWindowBackgroundBlurRadius`
symbols (used by `Window::set_blur`). Those are undocumented SPI; linking against them is a
rejection risk for the Mac App Store / notarisation review and produces linker warnings on
recent SDKs. Tabular never calls `set_blur`, so the extern declarations are removed. Also
drops the now-unused `NSInteger` / `AnyObject` imports.

## 2. `src/platform_impl/macos/window_delegate.rs` — `set_blur` is a no-op

Companion to (1): the body of `WindowDelegate::set_blur` that called the CGS SPI is replaced
by a no-op so the private symbols are not referenced anywhere. Behaviour change: window
background blur requests are silently ignored on macOS (Tabular does not use them).

## 3. `src/platform_impl/ios/view.rs` + `window.rs` + `Cargo.toml` — full `UITextInput` proxy

Upstream's `WinitView` only implements `UIKeyInput` (`insertText:` / `deleteBackward` /
`hasText`). On iPad that gives no autocorrect/predictive bar, no marked text (CJK input
methods, dictation), no text-interaction selection, and UIKit never learns where the caret
is, so the on-screen keyboard can cover it. The view now implements the whole `UITextInput`
protocol as a thin proxy (the real buffer lives in egui): a UTF-16 `Vec<u16>` mirror plus a
selection `NSRange` in the ivars, `WinitUITextPosition` / `WinitUITextRange` subclasses backed
by integer offsets, and sensible defaults for geometry queries. Mapping to winit events:

- committed text and backspace keep emitting `WindowEvent::KeyboardInput` exactly as before;
- `setMarkedText:selectedRange:` -> `Ime::Preedit(text, Some(byte range))`;
  `unmarkText` / `insertText:` while composing -> `Ime::Preedit("")` + `Ime::Commit(text)`
  (mirrors the macOS backend);
- `replaceRange:withText:` (autocorrect) -> N x Backspace key events + the new text;
- `becomeFirstResponder` / `resignFirstResponder` -> `Ime::Enabled` / `Ime::Disabled`,
  and the proxy buffer is cleared on resign;
- `Window::set_ime_cursor_area` (was a warning-only no-op on iOS) now stores the logical
  caret rect in the view, returned from `caretRectForPosition:` / `firstRectForRange:`.

`Cargo.toml` additionally enables the `NSRange` feature of `objc2-foundation` and `NSText`
of `objc2-ui-kit` (needed for the `UITextInput` bindings). No crate versions were bumped.
Limitations: `closestPositionToPoint:` / `characterRangeAtPoint:` return the current caret
(no layout info), `selectionRectsForRange:` is empty, writing direction is always natural.
