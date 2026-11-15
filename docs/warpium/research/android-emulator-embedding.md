# Android emulator in a pane and device mirroring (phase B)

Design notes, 2026-11-15. Phase A (`crates/warpium_android`, the Logcat pane) runs the emulator
in its own window. This note is what phase B needs to show the emulator, and physical devices,
inside a Warpium pane. Nothing here is built yet.

## Emulator: the gRPC endpoint

The emulator (since 30.x) serves `EmulatorController` over gRPC when started with `-grpc <port>`
(or `-grpc-use-token` / `-grpc-use-jwt`; Android Studio's embedded emulator uses
`-qt-hide-window -grpc-use-token -idle-grpc-timeout 300`). The discovery file
`$ANDROID_EMULATOR_HOME/avd/running/pid_<pid>.ini` (macOS:
`~/Library/Caches/TemporaryItems/avd/running/`) lists `grpc.port` and `grpc.token` for each
running emulator, so Warpium can also attach to an emulator Android Studio started.

What the pane needs from `emulator_controller.proto`:

- `streamScreenshot(ImageFormat)` → a stream of frames. `RGB888` at the pane's size (the request
  takes `width`/`height`, so the emulator scales) is the simple path; at 60 fps that is about
  100 MB/s on a 1080×2400 display, which is fine over loopback. The faster path is shared memory:
  `ImageFormat.transport = MMAP` with a file the emulator writes into, and the stream only carries
  frame metadata (Android Studio does this).
- `sendTouch` / `sendMouse` / `sendKey` / `sendWheel` for input (multi-touch, key codes from the
  Android `KeyEvent` table, IME text through `sendKey` with `text`).
- `getDisplayConfigurations`, `rotateVirtualSceneCamera`, `setPhysicalModel` (rotation, fold),
  `streamNotification` (display and posture changes), `getVmState` / `setVmState` (snapshots, pause).
- `streamAudio` is optional.

**Protoc-free constraint.** The app crate already needs `protoc`, but fork crates stay
protoc-free so they test in the container. Options: (1) commit the `prost`-generated Rust for
`emulator_controller.proto` (it changes rarely; regenerate with a script on a machine that has
`protoc`), or (2) `prost-reflect`/hand-written messages for the dozen messages needed. Transport
is `tonic` over HTTP/2 on loopback with the token as a `Bearer` header. Prefer (1).

**Rendering.** Decode nothing (RGB888/RGBA8888 frames) and upload into a texture the pane draws.
WarpUI draws images through its asset cache; a streaming texture needs a new element backed by a
GPU texture that is updated in place per frame instead of creating an image per frame. That
element is the main UI work of phase B. Throttle to the display refresh rate and drop frames
when the pane is hidden.

**Lifecycle.** Launch with `-qt-hide-window -grpc <free port> -grpc-use-token` when the user wants
the embedded mode (a setting per AVD or global), so no second window appears. Keep the phase-A
own-window launch as the default until the embedded pane is solid. Toolbar controls: power,
volume, rotate, back/home/overview (`sendKey` with `KEYCODE_*`), screenshot, snapshot.

## Physical devices: scrcpy-style mirroring

scrcpy pushes a small server jar to `/data/local/tmp`, starts it with `app_process` over
`adb shell`, and reverse-tunnels (`adb reverse localabstract:scrcpy tcp:<port>`, or forward when
reverse isn't available) one socket for H.264/H.265/AV1 video (from `MediaCodec`), one for audio
and one for control messages (touch, keys, clipboard, rotation). Its server is Apache-2.0, so
Warpium could ship the released `scrcpy-server` jar and implement the client side:

- the adb parts are already in `warpium_android` (`host:transport` + `shell:`; add `reverse:` and
  `sync:` for pushing the jar — `sync:` is the adb file-transfer protocol, `SEND` + `DATA` chunks +
  `DONE`),
- video decode needs hardware decoding to stay cheap: VideoToolbox on macOS
  (`VTDecompressionSession` producing `CVPixelBuffer`s that can be wrapped as Metal textures with
  `CVMetalTextureCache`, zero copy), behind a trait so other platforms can use `ffmpeg` later,
- the control protocol is a small binary format (scrcpy's `control_msg.c`), easy to port.

Android Studio's "Device Mirroring" works the same way with its own agent; following scrcpy is
less work and well documented.

## What phase A already provides

- device list, selection and boot waiting (`AndroidModel`, `tracker`, `avd`),
- the adb socket client to extend with `reverse:`/`sync:`,
- the pane plumbing (`IPaneType`, `LeafContents`, a pane manager per window) to copy for an
  emulator pane,
- the selected device shared by run, Logcat and (later) the device pane.

## Open questions

- A streaming-texture element in WarpUI (Metal on macOS; wgpu elsewhere) — needs a small spike.
- Whether to ship the scrcpy server jar in the app bundle or download it on first use.
- Keyboard focus and shortcuts when the device pane is focused (which keys go to the device).
