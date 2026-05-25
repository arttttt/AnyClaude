//! GPU-accelerated terminal renderer.
//!
//! Phase 3.5 prototype: validates pixel-based scroll with Warp-style momentum
//! before the full term_gpu crate is fleshed out. See
//! `docs/design/gpu-terminal-scroll.md` for the design and
//! `examples/scroll_demo.rs` for the demo entry point.

pub mod instances;
pub mod pipeline;
pub mod renderer;
pub mod scroll;

pub use instances::{RectInstance, Uniforms};
pub use renderer::GpuRenderer;
pub use scroll::{
    decay_velocity, ScrollState, ScrollVelocity, GESTURE_END_TIMEOUT, MOMENTUM_FRAME_INTERVAL,
    MOMENTUM_MIN_VELOCITY, MOMENTUM_THRESHOLD, NUM_PIXELS_PER_LINE,
};
