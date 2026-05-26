//! Visual demo of `term_layout`.
//!
//! Shows a BSP `PanelTree` as coloured rectangles, with the focused
//! panel highlighted. Keyboard shortcuts mutate the tree; mouse drags
//! resize the divider underneath the cursor.
//!
//! ## Run
//!
//! ```bash
//! cargo run -p term_gpu --example layout_demo --release
//! ```
//!
//! ## Shortcuts
//!
//! - `Cmd + D` (or `Ctrl + D`): split focused panel vertically (new
//!   pane on the right).
//! - `Cmd + Shift + D`: split focused panel horizontally (new pane on
//!   the bottom).
//! - `Cmd + W`: close focused panel. Closing the last panel exits the
//!   demo.
//! - Mouse left-click on a panel: focus it.
//! - Mouse left-drag near a divider: resize it.

use std::sync::Arc;

use term_gpu::{GlyphInstance, GpuRenderer, RectInstance, RenderLayer};
use term_layout::{BranchId, Divider, PanelId, PanelTree, Rect, Split};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

const INITIAL_W: f32 = 960.0;
const INITIAL_H: f32 = 600.0;
/// Logical-pixel tolerance for "did the user click on a divider?".
const DIVIDER_HIT_TOLERANCE: f32 = 6.0;
/// Logical-pixel thickness of the focus border drawn around the
/// focused panel — kept slim so it reads as a hint, not a frame.
const FOCUS_BORDER: f32 = 2.0;
/// Logical-pixel thickness of the divider line.
const DIVIDER_THICKNESS: f32 = 2.0;
const DIVIDER_COLOR: [f32; 4] = [0.05, 0.05, 0.08, 1.0];
/// Semi-transparent white — visible against the muted panel colours,
/// but soft enough not to dominate the panel.
const FOCUS_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.45];

#[derive(Debug, Clone, Copy)]
struct DragState {
    branch: BranchId,
    split: Split,
    bounds: Rect,
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    tree: PanelTree,
    scale_factor: f32,
    modifiers: ModifiersState,
    cursor: Option<(f32, f32)>,
    drag: Option<DragState>,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            tree: PanelTree::new(INITIAL_W, INITIAL_H),
            scale_factor: 1.0,
            modifiers: ModifiersState::empty(),
            cursor: None,
            drag: None,
        }
    }

    fn logical_window_size(&self) -> (f32, f32) {
        if let Some(r) = self.renderer.as_ref() {
            let s = r.size();
            (
                s.width as f32 / self.scale_factor,
                s.height as f32 / self.scale_factor,
            )
        } else {
            (INITIAL_W, INITIAL_H)
        }
    }

    fn redraw(&self) {
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    fn divider_under(&self, x: f32, y: f32) -> Option<Divider> {
        self.tree.dividers().into_iter().find(|d| match d.split {
            Split::Horizontal => {
                x >= d.rect.x
                    && x < d.rect.x + d.rect.w
                    && (y - d.rect.y).abs() <= DIVIDER_HIT_TOLERANCE
            }
            Split::Vertical => {
                y >= d.rect.y
                    && y < d.rect.y + d.rect.h
                    && (x - d.rect.x).abs() <= DIVIDER_HIT_TOLERANCE
            }
        })
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: &Key) {
        // Treat Cmd (macOS) and Ctrl (other platforms) as equivalent
        // so the demo runs on every winit platform without a rebuild.
        let cmd = self.modifiers.super_key() || self.modifiers.control_key();
        if !cmd {
            return;
        }
        let Key::Character(c) = key else {
            return;
        };
        let shift = self.modifiers.shift_key();
        match c.as_str() {
            "d" | "D" => {
                let split = if shift {
                    Split::Horizontal
                } else {
                    Split::Vertical
                };
                let target = self.tree.focus();
                self.tree.split(target, split, 0.5);
                self.redraw();
            }
            "w" | "W" => {
                let target = self.tree.focus();
                self.tree.close(target);
                if self.tree.is_empty() {
                    event_loop.exit();
                } else {
                    self.redraw();
                }
            }
            _ => {}
        }
    }

    fn on_mouse_press(&mut self) {
        let Some((x, y)) = self.cursor else { return };
        if let Some(d) = self.divider_under(x, y) {
            self.drag = Some(DragState {
                branch: d.id,
                split: d.split,
                bounds: d.bounds,
            });
            return;
        }
        if let Some(id) = self.tree.hit_test(x, y) {
            if self.tree.set_focus(id) {
                self.redraw();
            }
        }
    }

    fn on_mouse_release(&mut self) {
        self.drag = None;
    }

    fn on_cursor_moved(&mut self, x: f32, y: f32) {
        self.cursor = Some((x, y));
        if let Some(drag) = self.drag {
            let new_ratio = match drag.split {
                Split::Horizontal => (y - drag.bounds.y) / drag.bounds.h,
                Split::Vertical => (x - drag.bounds.x) / drag.bounds.w,
            };
            self.tree.drag_divider(drag.branch, new_ratio);
            self.redraw();
        }
    }

    fn build_rects(&self) -> Vec<RectInstance> {
        let mut rects = Vec::new();
        let focused = self.tree.focus();
        for (id, rect) in self.tree.panels() {
            rects.push(RectInstance {
                pos: [rect.x, rect.y],
                size: [rect.w, rect.h],
                color: panel_color(id),
            });
            if id == focused {
                rects.extend(focus_border(rect));
            }
        }
        for d in self.tree.dividers() {
            rects.push(divider_strip(d));
        }
        rects
    }

    fn on_redraw(&mut self) {
        let rects = self.build_rects();
        let glyphs: Vec<GlyphInstance> = Vec::new();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        window.pre_present_notify();
        renderer.render(RenderLayer::rects_and_glyphs(&rects, &glyphs), None, 0.0);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("term_layout demo")
            .with_inner_size(LogicalSize::new(INITIAL_W, INITIAL_H));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let renderer = GpuRenderer::new(window.clone());
        self.scale_factor = renderer.scale_factor();
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(new_size);
                }
                let (lw, lh) = self.logical_window_size();
                self.tree.resize(lw, lh);
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                if let Some(r) = self.renderer.as_mut() {
                    r.set_scale_factor(self.scale_factor);
                }
                let (lw, lh) = self.logical_window_size();
                self.tree.resize(lw, lh);
                self.redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    event_loop.exit();
                    return;
                }
                self.handle_key(event_loop, &event.logical_key);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let PhysicalPosition { x, y } = position;
                let logical = (x as f32 / self.scale_factor, y as f32 / self.scale_factor);
                self.on_cursor_moved(logical.0, logical.1);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.on_mouse_press(),
                ElementState::Released => self.on_mouse_release(),
            },
            WindowEvent::RedrawRequested => self.on_redraw(),
            _ => {}
        }
    }
}

fn panel_color(id: PanelId) -> [f32; 4] {
    // Golden-angle stepping in hue keeps adjacent ids visually
    // distinct without a palette table.
    let h = (id.0 as f32 * 137.508_f32) % 360.0;
    hsv_to_rgb(h, 0.55, 0.65)
}

fn focus_border(rect: Rect) -> [RectInstance; 4] {
    let b = FOCUS_BORDER;
    [
        RectInstance {
            pos: [rect.x, rect.y],
            size: [rect.w, b],
            color: FOCUS_COLOR,
        },
        RectInstance {
            pos: [rect.x, rect.y + rect.h - b],
            size: [rect.w, b],
            color: FOCUS_COLOR,
        },
        RectInstance {
            pos: [rect.x, rect.y],
            size: [b, rect.h],
            color: FOCUS_COLOR,
        },
        RectInstance {
            pos: [rect.x + rect.w - b, rect.y],
            size: [b, rect.h],
            color: FOCUS_COLOR,
        },
    ]
}

fn divider_strip(d: Divider) -> RectInstance {
    let t = DIVIDER_THICKNESS;
    match d.split {
        Split::Horizontal => RectInstance {
            pos: [d.rect.x, d.rect.y - t * 0.5],
            size: [d.rect.w, t],
            color: DIVIDER_COLOR,
        },
        Split::Vertical => RectInstance {
            pos: [d.rect.x - t * 0.5, d.rect.y],
            size: [t, d.rect.h],
            color: DIVIDER_COLOR,
        },
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 4] {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r1 + m, g1 + m, b1 + m, 1.0]
}

fn main() {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop failed");
}
