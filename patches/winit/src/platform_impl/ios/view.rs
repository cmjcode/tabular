#![allow(clippy::unnecessary_cast)]
//! iOS `UIView` subclass that receives touches, gestures and text input.
//!
//! Text input is implemented as a thin *proxy* `UITextInput` (TABULAR local patch, see
//! `TABULAR_PATCHES.md`). The real text buffer lives in the application (egui), so the view
//! only mirrors what UIKit tells it: committed text is forwarded as `KeyboardInput` events
//! (as upstream did through `UIKeyInput`), marked/composed text is forwarded as
//! `Ime::Preedit` / `Ime::Commit`, and the document model exposed to UIKit is a small
//! `String` + selection kept in the ivars. Positions and ranges are plain UTF-16 offsets.
//! Implementing the full protocol (instead of just `UIKeyInput`) is what makes UIKit show the
//! autocorrect/predictive bar, support CJK input methods and dictation, and lets it learn the
//! caret rect (`Window::set_ime_cursor_area`) so the on-screen keyboard avoids it.
use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability, sel};
use objc2_foundation::{
    CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSArray, NSComparisonResult, NSInteger,
    NSObject, NSRange, NSSet, NSString,
};
use objc2_ui_kit::{
    UICoordinateSpace, UIEvent, UIForceTouchCapability, UIGestureRecognizer,
    UIGestureRecognizerDelegate, UIGestureRecognizerState, UIKeyInput, UIPanGestureRecognizer,
    UIPinchGestureRecognizer, UIResponder, UIRotationGestureRecognizer, UITapGestureRecognizer,
    UITextInput, UITextInputDelegate, UITextInputStringTokenizer, UITextInputTokenizer,
    UITextInputTraits, UITextLayoutDirection, UITextPosition, UITextRange, UITextSelectionRect,
    UITextStorageDirection, UITouch, UITouchPhase, UITouchType, UITraitEnvironment, UIView,
};

use super::app_state::{self, EventWrapper};
use super::window::WinitUIWindow;
use crate::dpi::PhysicalPosition;
use crate::event::{ElementState, Event, Force, Ime, KeyEvent, Touch, TouchPhase, WindowEvent};
use crate::keyboard::{Key, KeyCode, KeyLocation, NamedKey, NativeKeyCode, PhysicalKey};
use crate::platform_impl::KeyEventExtra;
use crate::platform_impl::platform::DEVICE_ID;
use crate::window::{WindowAttributes, WindowId as RootWindowId};

pub struct WinitViewState {
    pinch_gesture_recognizer: RefCell<Option<Retained<UIPinchGestureRecognizer>>>,
    doubletap_gesture_recognizer: RefCell<Option<Retained<UITapGestureRecognizer>>>,
    rotation_gesture_recognizer: RefCell<Option<Retained<UIRotationGestureRecognizer>>>,
    pan_gesture_recognizer: RefCell<Option<Retained<UIPanGestureRecognizer>>>,

    // for iOS delta references the start of the Gesture
    rotation_last_delta: Cell<CGFloat>,
    pinch_last_delta: Cell<CGFloat>,
    pan_last_delta: Cell<CGPoint>,

    // Proxy text document exposed to UIKit through `UITextInput` (UTF-16 units).
    text_buffer: RefCell<Vec<u16>>,
    selected_range: Cell<NSRange>,
    marked_range: Cell<Option<NSRange>>,
    input_delegate: RefCell<Option<Retained<ProtocolObject<dyn UITextInputDelegate>>>>,
    tokenizer: RefCell<Option<Retained<UITextInputStringTokenizer>>>,
    // Caret rect reported by `Window::set_ime_cursor_area`, in view (logical) coordinates.
    ime_cursor_rect: Cell<CGRect>,
    ime_enabled: Cell<bool>,
}

pub(crate) struct WinitTextPositionIvars {
    offset: Cell<usize>,
}

declare_class!(
    /// `UITextPosition` backed by a UTF-16 offset into the proxy buffer.
    pub(crate) struct WinitTextPosition;

    unsafe impl ClassType for WinitTextPosition {
        #[inherits(NSObject)]
        type Super = UITextPosition;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "WinitUITextPosition";
    }

    impl DeclaredClass for WinitTextPosition {
        type Ivars = WinitTextPositionIvars;
    }
);

impl WinitTextPosition {
    fn new(mtm: MainThreadMarker, offset: usize) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(WinitTextPositionIvars {
            offset: Cell::new(offset),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    fn offset(&self) -> usize {
        self.ivars().offset.get()
    }
}

pub(crate) struct WinitTextRangeIvars {
    start: Cell<usize>,
    end: Cell<usize>,
}

declare_class!(
    /// `UITextRange` backed by two UTF-16 offsets into the proxy buffer.
    pub(crate) struct WinitTextRange;

    unsafe impl ClassType for WinitTextRange {
        #[inherits(NSObject)]
        type Super = UITextRange;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "WinitUITextRange";
    }

    impl DeclaredClass for WinitTextRange {
        type Ivars = WinitTextRangeIvars;
    }

    unsafe impl WinitTextRange {
        #[method(isEmpty)]
        fn is_empty(&self) -> bool {
            self.ivars().start.get() == self.ivars().end.get()
        }

        #[method_id(start)]
        fn start(&self) -> Retained<UITextPosition> {
            let mtm = MainThreadMarker::new().unwrap();
            Retained::into_super(WinitTextPosition::new(mtm, self.ivars().start.get()))
        }

        #[method_id(end)]
        fn end(&self) -> Retained<UITextPosition> {
            let mtm = MainThreadMarker::new().unwrap();
            Retained::into_super(WinitTextPosition::new(mtm, self.ivars().end.get()))
        }
    }
);

impl WinitTextRange {
    fn new(mtm: MainThreadMarker, start: usize, end: usize) -> Retained<Self> {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let this = mtm.alloc().set_ivars(WinitTextRangeIvars {
            start: Cell::new(start),
            end: Cell::new(end),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    fn from_ns_range(mtm: MainThreadMarker, range: NSRange) -> Retained<Self> {
        Self::new(mtm, range.location, range.location + range.length)
    }
}

/// Reads the offset of a position handed back by UIKit (always one we created).
fn position_offset(position: &UITextPosition) -> usize {
    if position.is_kind_of::<WinitTextPosition>() {
        let position: &WinitTextPosition =
            unsafe { &*(position as *const UITextPosition as *const WinitTextPosition) };
        position.offset()
    } else {
        0
    }
}

fn range_offsets(range: &UITextRange) -> (usize, usize) {
    if range.is_kind_of::<WinitTextRange>() {
        let range: &WinitTextRange =
            unsafe { &*(range as *const UITextRange as *const WinitTextRange) };
        (range.ivars().start.get(), range.ivars().end.get())
    } else {
        (0, 0)
    }
}

declare_class!(
    pub(crate) struct WinitView;

    unsafe impl ClassType for WinitView {
        #[inherits(UIResponder, NSObject)]
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "WinitUIView";
    }

    impl DeclaredClass for WinitView {
        type Ivars = WinitViewState;
    }

    unsafe impl WinitView {
        #[method(drawRect:)]
        fn draw_rect(&self, rect: CGRect) {
            let mtm = MainThreadMarker::new().unwrap();
            let window = self.window().unwrap();
            app_state::handle_nonuser_event(
                mtm,
                EventWrapper::StaticEvent(Event::WindowEvent {
                    window_id: RootWindowId(window.id()),
                    event: WindowEvent::RedrawRequested,
                }),
            );
            let _: () = unsafe { msg_send![super(self), drawRect: rect] };
        }

        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let mtm = MainThreadMarker::new().unwrap();
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };

            let window = self.window().unwrap();
            let window_bounds = window.bounds();
            let screen = window.screen();
            let screen_space = screen.coordinateSpace();
            let screen_frame = self.convertRect_toCoordinateSpace(window_bounds, &screen_space);
            let scale_factor = screen.scale();
            let size = crate::dpi::LogicalSize {
                width: screen_frame.size.width as f64,
                height: screen_frame.size.height as f64,
            }
            .to_physical(scale_factor as f64);

            // If the app is started in landscape, the view frame and window bounds can be mismatched.
            // The view frame will be in portrait and the window bounds in landscape. So apply the
            // window bounds to the view frame to make it consistent.
            let view_frame = self.frame();
            if view_frame != window_bounds {
                self.setFrame(window_bounds);
            }

            app_state::handle_nonuser_event(
                mtm,
                EventWrapper::StaticEvent(Event::WindowEvent {
                    window_id: RootWindowId(window.id()),
                    event: WindowEvent::Resized(size),
                }),
            );
        }

        #[method(setContentScaleFactor:)]
        fn set_content_scale_factor(&self, untrusted_scale_factor: CGFloat) {
            let mtm = MainThreadMarker::new().unwrap();
            let _: () =
                unsafe { msg_send![super(self), setContentScaleFactor: untrusted_scale_factor] };

            // `window` is null when `setContentScaleFactor` is invoked prior to `[UIWindow
            // makeKeyAndVisible]` at window creation time (either manually or internally by
            // UIKit when the `UIView` is first created), in which case we send no events here
            let window = match self.window() {
                Some(window) => window,
                None => return,
            };
            // `setContentScaleFactor` may be called with a value of 0, which means "reset the
            // content scale factor to a device-specific default value", so we can't use the
            // parameter here. We can query the actual factor using the getter
            let scale_factor = self.contentScaleFactor();
            assert!(
                !scale_factor.is_nan()
                    && scale_factor.is_finite()
                    && scale_factor.is_sign_positive()
                    && scale_factor > 0.0,
                "invalid scale_factor set on UIView",
            );
            let scale_factor = scale_factor as f64;
            let bounds = self.bounds();
            let screen = window.screen();
            let screen_space = screen.coordinateSpace();
            let screen_frame = self.convertRect_toCoordinateSpace(bounds, &screen_space);
            let size = crate::dpi::LogicalSize {
                width: screen_frame.size.width as f64,
                height: screen_frame.size.height as f64,
            };
            let window_id = RootWindowId(window.id());
            app_state::handle_nonuser_events(
                mtm,
                std::iter::once(EventWrapper::ScaleFactorChanged(
                    app_state::ScaleFactorChanged {
                        window,
                        scale_factor,
                        suggested_size: size.to_physical(scale_factor),
                    },
                ))
                .chain(std::iter::once(EventWrapper::StaticEvent(
                    Event::WindowEvent {
                        window_id,
                        event: WindowEvent::Resized(size.to_physical(scale_factor)),
                    },
                ))),
            );
        }

        #[method(touchesBegan:withEvent:)]
        fn touches_began(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.handle_touches(touches)
        }

        #[method(touchesMoved:withEvent:)]
        fn touches_moved(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.handle_touches(touches)
        }

        #[method(touchesEnded:withEvent:)]
        fn touches_ended(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.handle_touches(touches)
        }

        #[method(touchesCancelled:withEvent:)]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.handle_touches(touches)
        }

        #[method(pinchGesture:)]
        fn pinch_gesture(&self, recognizer: &UIPinchGestureRecognizer) {
            let window = self.window().unwrap();

            let (phase, delta) = match recognizer.state() {
                UIGestureRecognizerState::Began => {
                    self.ivars().pinch_last_delta.set(recognizer.scale());
                    (TouchPhase::Started, 0.0)
                }
                UIGestureRecognizerState::Changed => {
                    let last_scale: f64 = self.ivars().pinch_last_delta.replace(recognizer.scale());
                    (TouchPhase::Moved, recognizer.scale() - last_scale)
                }
                UIGestureRecognizerState::Ended => {
                    let last_scale: f64 = self.ivars().pinch_last_delta.replace(0.0);
                    (TouchPhase::Moved, recognizer.scale() - last_scale)
                }
                UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => {
                    self.ivars().rotation_last_delta.set(0.0);
                    // Pass -delta so that action is reversed
                    (TouchPhase::Cancelled, -recognizer.scale())
                }
                state => panic!("unexpected recognizer state: {state:?}"),
            };

            let gesture_event = EventWrapper::StaticEvent(Event::WindowEvent {
                window_id: RootWindowId(window.id()),
                event: WindowEvent::PinchGesture {
                    device_id: DEVICE_ID,
                    delta: delta as f64,
                    phase,
                },
            });

            let mtm = MainThreadMarker::new().unwrap();
            app_state::handle_nonuser_event(mtm, gesture_event);
        }

        #[method(doubleTapGesture:)]
        fn double_tap_gesture(&self, recognizer: &UITapGestureRecognizer) {
            let window = self.window().unwrap();

            if recognizer.state() == UIGestureRecognizerState::Ended {
                let gesture_event = EventWrapper::StaticEvent(Event::WindowEvent {
                    window_id: RootWindowId(window.id()),
                    event: WindowEvent::DoubleTapGesture {
                        device_id: DEVICE_ID,
                    },
                });

                let mtm = MainThreadMarker::new().unwrap();
                app_state::handle_nonuser_event(mtm, gesture_event);
            }
        }

        #[method(rotationGesture:)]
        fn rotation_gesture(&self, recognizer: &UIRotationGestureRecognizer) {
            let window = self.window().unwrap();

            let (phase, delta) = match recognizer.state() {
                UIGestureRecognizerState::Began => {
                    self.ivars().rotation_last_delta.set(0.0);

                    (TouchPhase::Started, 0.0)
                }
                UIGestureRecognizerState::Changed => {
                    let last_rotation = self.ivars().rotation_last_delta.replace(recognizer.rotation());

                    (TouchPhase::Moved, recognizer.rotation() - last_rotation)
                }
                UIGestureRecognizerState::Ended => {
                    let last_rotation = self.ivars().rotation_last_delta.replace(0.0);

                    (TouchPhase::Ended, recognizer.rotation() - last_rotation)
                }
                UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => {
                    self.ivars().rotation_last_delta.set(0.0);

                    // Pass -delta so that action is reversed
                    (TouchPhase::Cancelled, -recognizer.rotation())
                }
                state => panic!("unexpected recognizer state: {state:?}"),
            };

            // Make delta negative to match macos, convert to degrees
            let gesture_event = EventWrapper::StaticEvent(Event::WindowEvent {
                window_id: RootWindowId(window.id()),
                event: WindowEvent::RotationGesture {
                    device_id: DEVICE_ID,
                    delta: -delta.to_degrees() as _,
                    phase,
                },
            });

            let mtm = MainThreadMarker::new().unwrap();
            app_state::handle_nonuser_event(mtm, gesture_event);
        }

        #[method(panGesture:)]
        fn pan_gesture(&self, recognizer: &UIPanGestureRecognizer) {
            let window = self.window().unwrap();

            let translation = recognizer.translationInView(Some(self));

            let (phase, dx, dy) = match recognizer.state() {
                UIGestureRecognizerState::Began => {
                    self.ivars().pan_last_delta.set(translation);

                    (TouchPhase::Started, 0.0, 0.0)
                }
                UIGestureRecognizerState::Changed => {
                    let last_pan: CGPoint = self.ivars().pan_last_delta.replace(translation);

                    let dx = translation.x - last_pan.x;
                    let dy = translation.y - last_pan.y;

                    (TouchPhase::Moved, dx, dy)
                }
                UIGestureRecognizerState::Ended => {
                    let last_pan: CGPoint = self.ivars().pan_last_delta.replace(CGPoint{x:0.0, y:0.0});

                    let dx = translation.x - last_pan.x;
                    let dy = translation.y - last_pan.y;

                    (TouchPhase::Ended, dx, dy)
                }
                UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => {
                    let last_pan: CGPoint = self.ivars().pan_last_delta.replace(CGPoint{x:0.0, y:0.0});

                    // Pass -delta so that action is reversed
                    (TouchPhase::Cancelled, -last_pan.x, -last_pan.y)
                }
                state => panic!("unexpected recognizer state: {state:?}"),
            };


            let gesture_event = EventWrapper::StaticEvent(Event::WindowEvent {
                window_id: RootWindowId(window.id()),
                event: WindowEvent::PanGesture {
                    device_id: DEVICE_ID,
                    delta: PhysicalPosition::new(dx as _, dy as _),
                    phase,
                },
            });

            let mtm = MainThreadMarker::new().unwrap();
            app_state::handle_nonuser_event(mtm, gesture_event);
        }

        #[method(canBecomeFirstResponder)]
        fn can_become_first_responder(&self) -> bool {
            true
        }
    }

    unsafe impl NSObjectProtocol for WinitView {}

    unsafe impl UIGestureRecognizerDelegate for WinitView {
        #[method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:)]
        fn should_recognize_simultaneously(&self, _gesture_recognizer: &UIGestureRecognizer, _other_gesture_recognizer: &UIGestureRecognizer) -> bool {
            true
        }
    }

    unsafe impl UITextInputTraits for WinitView {
    }

    unsafe impl UIKeyInput for WinitView {
        #[method(hasText)]
        fn has_text(&self) -> bool {
            true
        }

        #[method(insertText:)]
        fn insert_text(&self, text: &NSString) {
            // Committing while composing: the composed text is replaced by `text`, which
            // we forward as a commit (mirrors the macOS backend's `insertText:`).
            if let Some(range) = self.ivars().marked_range.take() {
                self.replace_proxy(range, text);
                self.send_ime_event(Ime::Preedit(String::new(), None));
                self.send_ime_event(Ime::Commit(text.to_string()));
                return;
            }
            let sel = self.ivars().selected_range.get();
            self.replace_proxy(sel, text);
            self.handle_insert_text(text)
        }

        #[method(deleteBackward)]
        fn delete_backward(&self) {
            // Deleting while composing only edits the marked text; UIKit will follow up
            // with a new `setMarkedText:` call.
            if self.ivars().marked_range.get().is_some() {
                return;
            }
            self.handle_delete_backward()
        }
    }

    unsafe impl WinitView {
        #[method(becomeFirstResponder)]
        fn become_first_responder(&self) -> bool {
            let ok: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if ok && !self.ivars().ime_enabled.replace(true) {
                self.send_ime_event(Ime::Enabled);
            }
            ok
        }

        #[method(resignFirstResponder)]
        fn resign_first_responder(&self) -> bool {
            let ok: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if ok && self.ivars().ime_enabled.replace(false) {
                self.clear_marked_text();
                // The application owns the real text; the proxy is only a per-focus mirror.
                self.ivars().text_buffer.borrow_mut().clear();
                self.ivars().selected_range.set(NSRange::new(0, 0));
                self.send_ime_event(Ime::Disabled);
            }
            ok
        }
    }

    unsafe impl UITextInput for WinitView {
        #[method_id(textInRange:)]
        fn text_in_range(&self, range: &UITextRange) -> Option<Retained<NSString>> {
            let (start, end) = range_offsets(range);
            let buf = self.ivars().text_buffer.borrow();
            let end = end.min(buf.len());
            let start = start.min(end);
            Some(NSString::from_str(&String::from_utf16_lossy(&buf[start..end])))
        }

        #[method(replaceRange:withText:)]
        fn replace_range_with_text(&self, range: &UITextRange, text: &NSString) {
            // Autocorrect / predictive replacements: delete the old characters, then
            // insert the replacement, so the application sees ordinary key events.
            let (start, end) = range_offsets(range);
            self.replace_proxy(NSRange::new(start, end.saturating_sub(start)), text);
            for _ in start..end {
                self.handle_delete_backward();
            }
            self.handle_insert_text(text);
        }

        #[method_id(selectedTextRange)]
        fn selected_text_range(&self) -> Option<Retained<UITextRange>> {
            let mtm = MainThreadMarker::new().unwrap();
            Some(Retained::into_super(WinitTextRange::from_ns_range(mtm, self.ivars().selected_range.get())))
        }

        #[method(setSelectedTextRange:)]
        fn set_selected_text_range(&self, range: Option<&UITextRange>) {
            if let Some(range) = range {
                let (start, end) = range_offsets(range);
                self.ivars().selected_range.set(NSRange::new(start, end - start));
            }
        }

        #[method_id(markedTextRange)]
        fn marked_text_range(&self) -> Option<Retained<UITextRange>> {
            let mtm = MainThreadMarker::new().unwrap();
            self.ivars().marked_range.get().map(|r| Retained::into_super(WinitTextRange::from_ns_range(mtm, r)))
        }

        #[method_id(markedTextStyle)]
        fn marked_text_style(&self) -> Option<Retained<AnyObject>> {
            None
        }

        #[method(setMarkedTextStyle:)]
        fn set_marked_text_style(&self, _style: Option<&AnyObject>) {}

        #[method(setMarkedText:selectedRange:)]
        fn set_marked_text(&self, marked_text: Option<&NSString>, selected_range: NSRange) {
            let marked = marked_text.map(|s| s.to_string()).unwrap_or_default();
            let replace = self.ivars().marked_range.get().unwrap_or_else(|| self.ivars().selected_range.get());
            let marked_ns = NSString::from_str(&marked);
            self.replace_proxy(replace, &marked_ns);
            let len = marked_ns.length();
            self.ivars().marked_range.set(Some(NSRange::new(replace.location, len)));
            let sel_start = selected_range.location.min(len);
            let sel_end = (selected_range.location + selected_range.length).min(len);
            self.ivars().selected_range.set(NSRange::new(replace.location + sel_start, sel_end - sel_start));
            // `Ime::Preedit` cursor is in bytes of the preedit string.
            let utf16: Vec<u16> = marked.encode_utf16().collect();
            let byte_at = |units: usize| String::from_utf16_lossy(&utf16[..units.min(utf16.len())]).len();
            self.send_ime_event(Ime::Preedit(marked, Some((byte_at(sel_start), byte_at(sel_end)))));
        }

        #[method(unmarkText)]
        fn unmark_text(&self) {
            if let Some(range) = self.ivars().marked_range.take() {
                let text = {
                    let buf = self.ivars().text_buffer.borrow();
                    let end = (range.location + range.length).min(buf.len());
                    String::from_utf16_lossy(&buf[range.location.min(end)..end])
                };
                self.ivars().selected_range.set(NSRange::new(range.location + range.length, 0));
                self.send_ime_event(Ime::Preedit(String::new(), None));
                if !text.is_empty() {
                    self.send_ime_event(Ime::Commit(text));
                }
            }
        }

        #[method_id(beginningOfDocument)]
        fn beginning_of_document(&self) -> Retained<UITextPosition> {
            let mtm = MainThreadMarker::new().unwrap();
            Retained::into_super(WinitTextPosition::new(mtm, 0))
        }

        #[method_id(endOfDocument)]
        fn end_of_document(&self) -> Retained<UITextPosition> {
            let mtm = MainThreadMarker::new().unwrap();
            Retained::into_super(WinitTextPosition::new(mtm, self.ivars().text_buffer.borrow().len()))
        }

        #[method_id(textRangeFromPosition:toPosition:)]
        fn text_range_from_position(&self, from: &UITextPosition, to: &UITextPosition) -> Option<Retained<UITextRange>> {
            let mtm = MainThreadMarker::new().unwrap();
            Some(Retained::into_super(WinitTextRange::new(mtm, position_offset(from), position_offset(to))))
        }

        #[method_id(positionFromPosition:offset:)]
        fn position_from_position_offset(&self, position: &UITextPosition, offset: NSInteger) -> Option<Retained<UITextPosition>> {
            self.proxy_position_offset(position, offset)
        }

        #[method_id(positionFromPosition:inDirection:offset:)]
        fn position_from_position_in_direction(&self, position: &UITextPosition, direction: UITextLayoutDirection, offset: NSInteger) -> Option<Retained<UITextPosition>> {
            let signed = match direction {
                UITextLayoutDirection::Left | UITextLayoutDirection::Up => -offset,
                _ => offset,
            };
            self.proxy_position_offset(position, signed)
        }

        #[method(comparePosition:toPosition:)]
        fn compare_position(&self, a: &UITextPosition, b: &UITextPosition) -> NSComparisonResult {
            match position_offset(a).cmp(&position_offset(b)) {
                std::cmp::Ordering::Less => NSComparisonResult::Ascending,
                std::cmp::Ordering::Equal => NSComparisonResult::Same,
                std::cmp::Ordering::Greater => NSComparisonResult::Descending,
            }
        }

        #[method(offsetFromPosition:toPosition:)]
        fn offset_from_position(&self, from: &UITextPosition, to: &UITextPosition) -> NSInteger {
            position_offset(to) as NSInteger - position_offset(from) as NSInteger
        }

        #[method_id(inputDelegate)]
        fn input_delegate(&self) -> Option<Retained<ProtocolObject<dyn UITextInputDelegate>>> {
            // SAFETY: retaining an object we already hold a strong reference to.
            self.ivars().input_delegate.borrow().as_ref().and_then(|d| unsafe {
                Retained::retain(Retained::as_ptr(d) as *mut ProtocolObject<dyn UITextInputDelegate>)
            })
        }

        #[method(setInputDelegate:)]
        fn set_input_delegate(&self, delegate: Option<&ProtocolObject<dyn UITextInputDelegate>>) {
            *self.ivars().input_delegate.borrow_mut() = delegate.and_then(|d| unsafe {
                // SAFETY: `d` is a live object handed to us by UIKit; we take our own +1.
                Retained::retain(d as *const _ as *mut ProtocolObject<dyn UITextInputDelegate>)
            });
        }

        #[method_id(tokenizer)]
        fn tokenizer(&self) -> Retained<ProtocolObject<dyn UITextInputTokenizer>> {
            let mut slot = self.ivars().tokenizer.borrow_mut();
            let tokenizer = slot.get_or_insert_with(|| {
                let mtm = MainThreadMarker::new().unwrap();
                let responder: &UIResponder = self;
                unsafe { UITextInputStringTokenizer::initWithTextInput(mtm.alloc(), responder) }
            });
            ProtocolObject::from_retained(tokenizer.clone())
        }

        #[method_id(positionWithinRange:farthestInDirection:)]
        fn position_within_range_farthest(&self, range: &UITextRange, direction: UITextLayoutDirection) -> Option<Retained<UITextPosition>> {
            let mtm = MainThreadMarker::new().unwrap();
            let (start, end) = range_offsets(range);
            let offset = match direction {
                UITextLayoutDirection::Left | UITextLayoutDirection::Up => start,
                _ => end,
            };
            Some(Retained::into_super(WinitTextPosition::new(mtm, offset)))
        }

        #[method_id(characterRangeByExtendingPosition:inDirection:)]
        fn character_range_by_extending(&self, position: &UITextPosition, direction: UITextLayoutDirection) -> Option<Retained<UITextRange>> {
            let mtm = MainThreadMarker::new().unwrap();
            let offset = position_offset(position);
            let len = self.ivars().text_buffer.borrow().len();
            let (start, end) = match direction {
                UITextLayoutDirection::Left | UITextLayoutDirection::Up => (0, offset),
                _ => (offset, len),
            };
            Some(Retained::into_super(WinitTextRange::new(mtm, start, end)))
        }

        #[method(baseWritingDirectionForPosition:inDirection:)]
        fn base_writing_direction(&self, _position: &UITextPosition, _direction: UITextStorageDirection) -> NSInteger {
            // NSWritingDirectionNatural
            -1
        }

        #[method(setBaseWritingDirection:forRange:)]
        fn set_base_writing_direction(&self, _direction: NSInteger, _range: &UITextRange) {}

        #[method(firstRectForRange:)]
        fn first_rect_for_range(&self, _range: &UITextRange) -> CGRect {
            self.ivars().ime_cursor_rect.get()
        }

        #[method(caretRectForPosition:)]
        fn caret_rect_for_position(&self, _position: &UITextPosition) -> CGRect {
            self.ivars().ime_cursor_rect.get()
        }

        #[method_id(selectionRectsForRange:)]
        fn selection_rects_for_range(&self, _range: &UITextRange) -> Retained<NSArray<UITextSelectionRect>> {
            NSArray::new()
        }

        #[method_id(closestPositionToPoint:)]
        fn closest_position_to_point(&self, _point: CGPoint) -> Option<Retained<UITextPosition>> {
            // No layout information: report the current caret.
            let mtm = MainThreadMarker::new().unwrap();
            Some(Retained::into_super(WinitTextPosition::new(mtm, self.ivars().selected_range.get().location)))
        }

        #[method_id(closestPositionToPoint:withinRange:)]
        fn closest_position_to_point_within_range(&self, _point: CGPoint, range: &UITextRange) -> Option<Retained<UITextPosition>> {
            let mtm = MainThreadMarker::new().unwrap();
            let (start, end) = range_offsets(range);
            let caret = self.ivars().selected_range.get().location.clamp(start, end);
            Some(Retained::into_super(WinitTextPosition::new(mtm, caret)))
        }

        #[method_id(characterRangeAtPoint:)]
        fn character_range_at_point(&self, _point: CGPoint) -> Option<Retained<UITextRange>> {
            let mtm = MainThreadMarker::new().unwrap();
            let sel = self.ivars().selected_range.get();
            Some(Retained::into_super(WinitTextRange::from_ns_range(mtm, sel)))
        }
    }
);

impl WinitView {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        window_attributes: &WindowAttributes,
        frame: CGRect,
    ) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(WinitViewState {
            pinch_gesture_recognizer: RefCell::new(None),
            doubletap_gesture_recognizer: RefCell::new(None),
            rotation_gesture_recognizer: RefCell::new(None),
            pan_gesture_recognizer: RefCell::new(None),

            rotation_last_delta: Cell::new(0.0),
            pinch_last_delta: Cell::new(0.0),
            pan_last_delta: Cell::new(CGPoint { x: 0.0, y: 0.0 }),

            text_buffer: RefCell::new(Vec::new()),
            selected_range: Cell::new(NSRange::new(0, 0)),
            marked_range: Cell::new(None),
            input_delegate: RefCell::new(None),
            tokenizer: RefCell::new(None),
            ime_cursor_rect: Cell::new(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1.0, 20.0))),
            ime_enabled: Cell::new(false),
        });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), initWithFrame: frame] };

        this.setMultipleTouchEnabled(true);

        if let Some(scale_factor) = window_attributes.platform_specific.scale_factor {
            this.setContentScaleFactor(scale_factor as _);
        }

        this
    }

    fn window(&self) -> Option<Retained<WinitUIWindow>> {
        // SAFETY: `WinitView`s are always installed in a `WinitUIWindow`
        (**self)
            .window()
            .map(|window| unsafe { Retained::cast(window) })
    }

    pub(crate) fn recognize_pinch_gesture(&self, should_recognize: bool) {
        let mtm = MainThreadMarker::from(self);
        if should_recognize {
            if self.ivars().pinch_gesture_recognizer.borrow().is_none() {
                let pinch = unsafe {
                    UIPinchGestureRecognizer::initWithTarget_action(
                        mtm.alloc(),
                        Some(self),
                        Some(sel!(pinchGesture:)),
                    )
                };
                pinch.setDelegate(Some(ProtocolObject::from_ref(self)));
                self.addGestureRecognizer(&pinch);
                self.ivars().pinch_gesture_recognizer.replace(Some(pinch));
            }
        } else if let Some(recognizer) = self.ivars().pinch_gesture_recognizer.take() {
            self.removeGestureRecognizer(&recognizer);
        }
    }

    pub(crate) fn recognize_pan_gesture(
        &self,
        should_recognize: bool,
        minimum_number_of_touches: u8,
        maximum_number_of_touches: u8,
    ) {
        let mtm = MainThreadMarker::from(self);
        if should_recognize {
            if self.ivars().pan_gesture_recognizer.borrow().is_none() {
                let pan = unsafe {
                    UIPanGestureRecognizer::initWithTarget_action(
                        mtm.alloc(),
                        Some(self),
                        Some(sel!(panGesture:)),
                    )
                };
                pan.setDelegate(Some(ProtocolObject::from_ref(self)));
                pan.setMinimumNumberOfTouches(minimum_number_of_touches as _);
                pan.setMaximumNumberOfTouches(maximum_number_of_touches as _);
                self.addGestureRecognizer(&pan);
                self.ivars().pan_gesture_recognizer.replace(Some(pan));
            }
        } else if let Some(recognizer) = self.ivars().pan_gesture_recognizer.take() {
            self.removeGestureRecognizer(&recognizer);
        }
    }

    pub(crate) fn recognize_doubletap_gesture(&self, should_recognize: bool) {
        let mtm = MainThreadMarker::from(self);
        if should_recognize {
            if self.ivars().doubletap_gesture_recognizer.borrow().is_none() {
                let tap = unsafe {
                    UITapGestureRecognizer::initWithTarget_action(
                        mtm.alloc(),
                        Some(self),
                        Some(sel!(doubleTapGesture:)),
                    )
                };
                tap.setDelegate(Some(ProtocolObject::from_ref(self)));
                tap.setNumberOfTapsRequired(2);
                tap.setNumberOfTouchesRequired(1);
                self.addGestureRecognizer(&tap);
                self.ivars().doubletap_gesture_recognizer.replace(Some(tap));
            }
        } else if let Some(recognizer) = self.ivars().doubletap_gesture_recognizer.take() {
            self.removeGestureRecognizer(&recognizer);
        }
    }

    pub(crate) fn recognize_rotation_gesture(&self, should_recognize: bool) {
        let mtm = MainThreadMarker::from(self);
        if should_recognize {
            if self.ivars().rotation_gesture_recognizer.borrow().is_none() {
                let rotation = unsafe {
                    UIRotationGestureRecognizer::initWithTarget_action(
                        mtm.alloc(),
                        Some(self),
                        Some(sel!(rotationGesture:)),
                    )
                };
                rotation.setDelegate(Some(ProtocolObject::from_ref(self)));
                self.addGestureRecognizer(&rotation);
                self.ivars()
                    .rotation_gesture_recognizer
                    .replace(Some(rotation));
            }
        } else if let Some(recognizer) = self.ivars().rotation_gesture_recognizer.take() {
            self.removeGestureRecognizer(&recognizer);
        }
    }

    fn handle_touches(&self, touches: &NSSet<UITouch>) {
        let window = self.window().unwrap();
        let mut touch_events = Vec::new();
        let os_supports_force = app_state::os_capabilities().force_touch;
        for touch in touches {
            let logical_location = touch.locationInView(None);
            let touch_type = touch.r#type();
            let force = if os_supports_force {
                let trait_collection = self.traitCollection();
                let touch_capability = trait_collection.forceTouchCapability();
                // Both the OS _and_ the device need to be checked for force touch support.
                if touch_capability == UIForceTouchCapability::Available
                    || touch_type == UITouchType::Pencil
                {
                    let force = touch.force();
                    let max_possible_force = touch.maximumPossibleForce();
                    let altitude_angle: Option<f64> = if touch_type == UITouchType::Pencil {
                        let angle = touch.altitudeAngle();
                        Some(angle as _)
                    } else {
                        None
                    };
                    Some(Force::Calibrated {
                        force: force as _,
                        max_possible_force: max_possible_force as _,
                        altitude_angle,
                    })
                } else {
                    None
                }
            } else {
                None
            };
            let touch_id = touch as *const UITouch as u64;
            let phase = touch.phase();
            let phase = match phase {
                UITouchPhase::Began => TouchPhase::Started,
                UITouchPhase::Moved => TouchPhase::Moved,
                // 2 is UITouchPhase::Stationary and is not expected here
                UITouchPhase::Ended => TouchPhase::Ended,
                UITouchPhase::Cancelled => TouchPhase::Cancelled,
                _ => panic!("unexpected touch phase: {phase:?}"),
            };

            let physical_location = {
                let scale_factor = self.contentScaleFactor();
                PhysicalPosition::from_logical::<(f64, f64), f64>(
                    (logical_location.x as _, logical_location.y as _),
                    scale_factor as f64,
                )
            };
            touch_events.push(EventWrapper::StaticEvent(Event::WindowEvent {
                window_id: RootWindowId(window.id()),
                event: WindowEvent::Touch(Touch {
                    device_id: DEVICE_ID,
                    id: touch_id,
                    location: physical_location,
                    force,
                    phase,
                }),
            }));
        }
        let mtm = MainThreadMarker::new().unwrap();
        app_state::handle_nonuser_events(mtm, touch_events);
    }

    fn handle_insert_text(&self, text: &NSString) {
        let window = self.window().unwrap();
        let window_id = RootWindowId(window.id());
        let mtm = MainThreadMarker::new().unwrap();
        // send individual events for each character
        app_state::handle_nonuser_events(
            mtm,
            text.to_string().chars().flat_map(|c| {
                let text = smol_str::SmolStr::from_iter([c]);
                // Emit both press and release events
                [ElementState::Pressed, ElementState::Released].map(|state| {
                    EventWrapper::StaticEvent(Event::WindowEvent {
                        window_id,
                        event: WindowEvent::KeyboardInput {
                            event: KeyEvent {
                                text: if state == ElementState::Pressed {
                                    Some(text.clone())
                                } else {
                                    None
                                },
                                state,
                                location: KeyLocation::Standard,
                                repeat: false,
                                logical_key: Key::Character(text.clone()),
                                physical_key: PhysicalKey::Unidentified(
                                    NativeKeyCode::Unidentified,
                                ),
                                platform_specific: KeyEventExtra {},
                            },
                            is_synthetic: false,
                            device_id: DEVICE_ID,
                        },
                    })
                })
            }),
        );
    }

    /// Called from `Window::set_ime_cursor_area` (logical, view coordinates).
    pub(crate) fn set_ime_cursor_rect(&self, rect: CGRect) {
        self.ivars().ime_cursor_rect.set(rect);
    }

    fn proxy_position_offset(
        &self,
        position: &UITextPosition,
        offset: NSInteger,
    ) -> Option<Retained<UITextPosition>> {
        let mtm = MainThreadMarker::new().unwrap();
        let len = self.ivars().text_buffer.borrow().len() as isize;
        let target = position_offset(position) as isize + offset as isize;
        if target < 0 || target > len {
            None
        } else {
            Some(Retained::into_super(WinitTextPosition::new(
                mtm,
                target as usize,
            )))
        }
    }

    fn send_ime_event(&self, ime: Ime) {
        let Some(window) = self.window() else { return };
        let mtm = MainThreadMarker::new().unwrap();
        app_state::handle_nonuser_event(
            mtm,
            EventWrapper::StaticEvent(Event::WindowEvent {
                window_id: RootWindowId(window.id()),
                event: WindowEvent::Ime(ime),
            }),
        );
    }

    /// Replaces `range` of the proxy buffer with `text` and moves the caret after it.
    fn replace_proxy(&self, range: NSRange, text: &NSString) {
        let new: Vec<u16> = text.to_string().encode_utf16().collect();
        let mut buf = self.ivars().text_buffer.borrow_mut();
        let end = (range.location + range.length).min(buf.len());
        let start = range.location.min(end);
        let new_len = new.len();
        buf.splice(start..end, new);
        self.ivars()
            .selected_range
            .set(NSRange::new(start + new_len, 0));
    }

    fn clear_marked_text(&self) {
        if let Some(range) = self.ivars().marked_range.take() {
            self.replace_proxy(range, &NSString::new());
            self.send_ime_event(Ime::Preedit(String::new(), None));
        }
    }

    fn handle_delete_backward(&self) {
        {
            let sel = self.ivars().selected_range.get();
            let mut buf = self.ivars().text_buffer.borrow_mut();
            if sel.length > 0 {
                let end = (sel.location + sel.length).min(buf.len());
                buf.drain(sel.location.min(end)..end);
                self.ivars()
                    .selected_range
                    .set(NSRange::new(sel.location.min(end), 0));
            } else if sel.location > 0 && sel.location <= buf.len() {
                buf.remove(sel.location - 1);
                self.ivars()
                    .selected_range
                    .set(NSRange::new(sel.location - 1, 0));
            }
        }
        let window = self.window().unwrap();
        let window_id = RootWindowId(window.id());
        let mtm = MainThreadMarker::new().unwrap();
        app_state::handle_nonuser_events(
            mtm,
            [ElementState::Pressed, ElementState::Released].map(|state| {
                EventWrapper::StaticEvent(Event::WindowEvent {
                    window_id,
                    event: WindowEvent::KeyboardInput {
                        device_id: DEVICE_ID,
                        event: KeyEvent {
                            state,
                            logical_key: Key::Named(NamedKey::Backspace),
                            physical_key: PhysicalKey::Code(KeyCode::Backspace),
                            platform_specific: KeyEventExtra {},
                            repeat: false,
                            location: KeyLocation::Standard,
                            text: None,
                        },
                        is_synthetic: false,
                    },
                })
            }),
        );
    }
}
