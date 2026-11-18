//! Warpium: talks to running Android emulators over their built-in gRPC control endpoint, for
//! showing an emulator inside a Warpium pane.
//!
//! - [`discovery`] finds running emulators, their gRPC port and token, and their adb serial.
//! - [`client`] streams frames and sends input, buttons and rotation.
//! - [`frame_slot`] hands only the latest frame to the renderer.
//! - [`geometry`] maps pointer positions to display coordinates; [`keys`] names device buttons.
//!
//! The protobuf code in [`proto`] is generated ahead of time, so building this crate needs
//! neither `protoc` nor a build script. The design is in
//! `docs/warpium/research/embedded-emulator.md`.

pub mod client;
pub mod discovery;
pub mod frame_slot;
pub mod geometry;
pub mod keys;
pub mod proto;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use client::{ClientError, EmulatorClient, FrameStream, InputSender};
pub use discovery::{GrpcToken, RunningEmulator};
pub use frame_slot::{Frame, FrameSlot};
pub use geometry::DisplayRotation;
pub use keys::DeviceButton;
