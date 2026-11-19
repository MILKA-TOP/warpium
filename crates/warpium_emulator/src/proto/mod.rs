//! Messages and the tonic client (and, for tests, server) for `android.emulation.control`.
//!
//! `generated.rs` is committed; regenerate it with `script/warpium/gen-emulator-proto` after
//! updating `proto/emulator_controller.proto` (vendored unmodified from AOSP
//! `platform/external/qemu`, branch `emu-master-dev`, commit
//! `ae9d18d2b6261179fbd57fffec720a04f7bfb053`, path
//! `android/android-grpc/services/emulator-controller/proto/emulator_controller.proto`).
#![allow(clippy::all, clippy::pedantic, missing_docs, rustdoc::all)]

include!("generated.rs");
