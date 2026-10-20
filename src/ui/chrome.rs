//! Window chrome for the frameless window (ui/kit/chrome.slint): moving, resizing,
//! minimizing, maximizing and closing go to the window manager through winit.

use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::winit::window::ResizeDirection;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use crate::{AppState, AppWindow};

fn resize_direction(edge: &str) -> Option<ResizeDirection> {
    Some(match edge {
        "n" => ResizeDirection::North,
        "s" => ResizeDirection::South,
        "e" => ResizeDirection::East,
        "w" => ResizeDirection::West,
        "ne" => ResizeDirection::NorthEast,
        "nw" => ResizeDirection::NorthWest,
        "se" => ResizeDirection::SouthEast,
        "sw" => ResizeDirection::SouthWest,
        _ => return None,
    })
}

pub fn wire(w: &AppWindow) {
    let state = w.global::<AppState>();

    let weak = w.as_weak();
    state.on_window_drag(move || {
        if let Some(w) = weak.upgrade() {
            w.window().with_winit_window(|ww| {
                if let Err(e) = ww.drag_window() {
                    log::debug!("Window move not available: {e}");
                }
            });
        }
    });
    let weak = w.as_weak();
    state.on_window_resize(move |edge| {
        let (Some(w), Some(dir)) = (weak.upgrade(), resize_direction(&edge)) else { return };
        w.window().with_winit_window(|ww| {
            if let Err(e) = ww.drag_resize_window(dir) {
                log::debug!("Window resize not available: {e}");
            }
        });
    });
    let weak = w.as_weak();
    state.on_window_minimize(move || {
        if let Some(w) = weak.upgrade() {
            w.window().set_minimized(true);
        }
    });
    let weak = w.as_weak();
    state.on_window_toggle_maximize(move || {
        if let Some(w) = weak.upgrade() {
            let max = !w.window().is_maximized();
            w.window().set_maximized(max);
            w.global::<AppState>().set_window_maximized(max);
        }
    });
    // Same path as the window manager's close, so the cancel-install prompt still applies.
    let weak = w.as_weak();
    state.on_window_close(move || {
        if let Some(w) = weak.upgrade() {
            let _ = w.window().dispatch_event_with_result(slint::platform::WindowEvent::CloseRequested);
        }
    });

    // Follow maximize changes made outside the title bar (keyboard shortcuts, snapping).
    let weak = w.as_weak();
    w.window().on_winit_window_event(move |window, event| {
        if matches!(event, WindowEvent::Resized(_))
            && let Some(w) = weak.upgrade()
        {
            w.global::<AppState>().set_window_maximized(window.is_maximized());
        }
        EventResult::Propagate
    });
}
