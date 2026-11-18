//! Warpium's Android tooling, protoc-free and independent of the app: SDK discovery, a client for
//! the adb server's socket protocol (no `adb` process per request), live device tracking, AVDs,
//! logcat streaming, parsing and filtering, stack-trace frames, and reading Android projects
//! without Gradle.
//!
//! Everything that touches the network, processes or the file system blocks, so callers run it
//! on background threads; the long-running parts ([`tracker::spawn_device_tracker`],
//! [`logcat::spawn_logcat`]) own their thread and stop when their [`WorkerHandle`] is dropped.

pub mod adb;
pub mod avd;
pub mod device;
pub mod logcat;
pub mod project;
pub mod run;
pub mod sdk;
pub mod stack;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod tracker;
mod worker;

pub use adb::{AdbClient, AdbError};
pub use device::{Device, DeviceEntry, DeviceKind, DeviceProps, DeviceState};
pub use worker::WorkerHandle;
