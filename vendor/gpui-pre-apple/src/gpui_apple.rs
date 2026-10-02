#![cfg(target_os = "macos")]
//! Shared Apple platform support for GPUI.
//!
//! This crate contains the Metal renderer and GPU resource management shared
//! by GPUI's Apple platform backends.

mod metal_atlas;
pub mod metal_renderer;
// ZJ patch: low-memory renderer policy (see vendor/README.md).
pub mod zj_low_memory;
