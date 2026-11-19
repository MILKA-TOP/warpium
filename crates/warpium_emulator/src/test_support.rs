//! An in-process fake of the emulator's gRPC endpoint, built from the same generated code as the
//! client. It streams synthetic frames and records what clients sent.

use std::{
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use tokio::{
    net::TcpListener,
    sync::{oneshot, watch},
    task::JoinHandle,
};
use tokio_stream::{Stream, StreamExt, wrappers::TcpListenerStream};
use tonic::{Request, Response, Status, Streaming, codegen::BoxStream, transport::Server};

use crate::{
    discovery::{GrpcToken, RunningEmulator},
    geometry::{DisplayRotation, DisplaySize},
    proto::{
        Image, ImageFormat, InputEvent, KeyboardEvent, PhysicalModelValue, Rotation, VmRunState,
        emulator_controller_server::{EmulatorController, EmulatorControllerServer},
        image_format::ImgFormat,
        rotation::SkinRotation,
        vm_run_state::RunState,
    },
};

/// Bytes the fake returns for `getScreenshot(PNG)`.
pub const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

#[derive(Clone, Debug)]
pub struct FakeEmulatorConfig {
    /// When set, calls without `authorization: Bearer <token>` fail with `UNAUTHENTICATED`.
    pub token: Option<String>,
    pub display: DisplaySize,
    pub rotation: DisplayRotation,
    /// Frames each `streamScreenshot` call sends before it idles (as on a static screen).
    pub frames_per_stream: u32,
    /// Pause between frames, like a guest rendering at a fixed rate; `None` sends them at once.
    pub frame_interval: Option<Duration>,
}

impl Default for FakeEmulatorConfig {
    fn default() -> Self {
        Self {
            token: None,
            display: DisplaySize {
                width: 1080,
                height: 2400,
            },
            rotation: DisplayRotation::Portrait,
            frames_per_stream: 3,
            frame_interval: None,
        }
    }
}

/// What the fake received, in arrival order.
#[derive(Clone, Debug, Default)]
pub struct Recorded {
    pub authorization_headers: Vec<Option<String>>,
    pub input_events: Vec<InputEvent>,
    pub keys: Vec<KeyboardEvent>,
    pub physical_models: Vec<PhysicalModelValue>,
    pub vm_states_set: Vec<VmRunState>,
    pub get_vm_state_calls: usize,
}

#[derive(Debug)]
struct FakeState {
    config: FakeEmulatorConfig,
    recorded: Mutex<Recorded>,
    active_screenshot_streams: AtomicUsize,
    /// Set when the fake stops, so open streams fail the way a dying emulator's connection does
    /// (the server's graceful shutdown would otherwise wait for them forever).
    stopped: watch::Sender<bool>,
}

impl FakeState {
    fn record(&self, update: impl FnOnce(&mut Recorded)) {
        update(&mut self.recorded.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

pub struct FakeEmulator {
    addr: SocketAddr,
    state: Arc<FakeState>,
    shutdown: Option<oneshot::Sender<()>>,
    server: Option<JoinHandle<()>>,
}

impl FakeEmulator {
    /// Serves on an ephemeral loopback port. Must be called within a tokio runtime.
    pub async fn start(config: FakeEmulatorConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake emulator");
        let addr = listener.local_addr().expect("fake emulator address");
        let state = Arc::new(FakeState {
            config,
            recorded: Mutex::default(),
            active_screenshot_streams: AtomicUsize::new(0),
            stopped: watch::Sender::new(false),
        });
        let auth_state = state.clone();
        let service = EmulatorControllerServer::with_interceptor(
            FakeService {
                state: state.clone(),
            },
            move |request: Request<()>| check_auth(&auth_state, request),
        );
        let (shutdown, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let _ = Server::builder()
                .add_service(service)
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Self {
            addr,
            state,
            shutdown: Some(shutdown),
            server: Some(server),
        }
    }

    pub fn grpc_port(&self) -> u16 {
        self.addr.port()
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The contents of a `pid_<pid>.ini` discovery file pointing at this fake.
    pub fn discovery_file_contents(&self, console_port: u16) -> String {
        let token = self
            .state
            .config
            .token
            .as_deref()
            .map(|token| format!("grpc.token={token}\n"))
            .unwrap_or_default();
        format!(
            "port.serial={console_port}\nport.adb={}\navd.name=Fake_AVD\ncmdline=emulator -avd Fake_AVD\ngrpc.port={}\n{token}",
            console_port + 1,
            self.grpc_port(),
        )
    }

    pub fn recorded(&self) -> Recorded {
        self.state
            .recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `streamScreenshot` calls the client has not cancelled yet.
    pub fn active_screenshot_streams(&self) -> usize {
        self.state.active_screenshot_streams.load(Ordering::SeqCst)
    }

    /// Stops serving and waits until the port is closed.
    pub async fn stop(mut self) {
        self.stop_server().await;
    }

    async fn stop_server(&mut self) {
        self.state.stopped.send_replace(true);
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
    }
}

/// A [`FakeEmulator`] serving from a thread of its own, for callers without a tokio runtime (such
/// as the app's debug commands). Dropping it stops the server.
pub struct FakeEmulatorThread {
    entry: RunningEmulator,
    shutdown: Option<oneshot::Sender<()>>,
}

impl FakeEmulatorThread {
    /// Starts a fake that discovery would list as `emulator-<console_port>`.
    pub fn spawn(config: FakeEmulatorConfig, console_port: u16) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let token = config.token.clone();
        let (port_tx, port_rx) = std::sync::mpsc::channel();
        let (shutdown, shutdown_rx) = oneshot::channel::<()>();
        std::thread::Builder::new()
            .name(format!("warpium-fake-emulator-{console_port}"))
            .spawn(move || {
                runtime.block_on(async move {
                    let fake = FakeEmulator::start(config).await;
                    let _ = port_tx.send(fake.grpc_port());
                    let _ = shutdown_rx.await;
                    fake.stop().await;
                });
            })?;
        let grpc_port = port_rx
            .recv()
            .map_err(|_| std::io::Error::other("the fake emulator failed to start"))?;
        Ok(Self {
            entry: RunningEmulator {
                pid: std::process::id(),
                console_port,
                adb_port: None,
                grpc_port,
                grpc_token: token.map(GrpcToken::new),
                grpc_uses_tls: false,
                avd_name: Some(format!("Fake_AVD_{console_port}")),
                avd_dir: None,
                emulator_version: None,
                command_line: Some("emulator -avd Fake_AVD".to_owned()),
            },
            shutdown: Some(shutdown),
        })
    }

    /// How discovery would describe this fake.
    pub fn entry(&self) -> &RunningEmulator {
        &self.entry
    }
}

impl Drop for FakeEmulatorThread {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

impl Drop for FakeEmulator {
    fn drop(&mut self) {
        self.state.stopped.send_replace(true);
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

fn check_auth(state: &FakeState, request: Request<()>) -> Result<Request<()>, Status> {
    let header = request
        .metadata()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let authorized = match &state.config.token {
        Some(token) => header.as_deref() == Some(format!("Bearer {token}").as_str()),
        None => true,
    };
    state.record(|recorded| recorded.authorization_headers.push(header));
    if authorized {
        Ok(request)
    } else {
        Err(Status::unauthenticated("missing or invalid token"))
    }
}

/// The frame size the emulator sends for a request: the rotated display scaled to fit the
/// requested box, never larger than the display.
pub fn fake_frame_size(
    display: DisplaySize,
    rotation: DisplayRotation,
    requested_width: u32,
    requested_height: u32,
) -> (u32, u32) {
    let (width, height) = if rotation.is_landscape() {
        (display.height, display.width)
    } else {
        (display.width, display.height)
    };
    if requested_width == 0 || requested_height == 0 {
        return (width, height);
    }
    let scale = (requested_width as f64 / width as f64)
        .min(requested_height as f64 / height as f64)
        .min(1.0);
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

/// The synthetic pixel at `(x, y)` of frame `seq`: red and green follow the position, blue is the
/// frame number.
pub fn fake_pixel(x: u32, y: u32, seq: u32) -> [u8; 4] {
    [x as u8, y as u8, seq as u8, 0xff]
}

fn fake_frame(width: u32, height: u32, rotation: DisplayRotation, seq: u32) -> Image {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&fake_pixel(x, y, seq));
        }
    }
    let skin_rotation = match rotation {
        DisplayRotation::Portrait => SkinRotation::Portrait,
        DisplayRotation::Landscape => SkinRotation::Landscape,
        DisplayRotation::ReversePortrait => SkinRotation::ReversePortrait,
        DisplayRotation::ReverseLandscape => SkinRotation::ReverseLandscape,
    };
    Image {
        format: Some(ImageFormat {
            format: ImgFormat::Rgba8888 as i32,
            rotation: Some(Rotation {
                rotation: skin_rotation as i32,
                ..Default::default()
            }),
            width,
            height,
            ..Default::default()
        }),
        image: Bytes::from(pixels),
        seq,
        timestamp_us: u64::from(seq) * 16_000,
        ..Default::default()
    }
}

/// Keeps the active-stream counter accurate: the server drops the response stream when the client
/// cancels the call.
struct CountedStream {
    inner: BoxStream<Image>,
    state: Arc<FakeState>,
    stopped: Pin<Box<dyn Future<Output = ()> + Send>>,
    ended: bool,
}

impl CountedStream {
    fn new(inner: BoxStream<Image>, state: Arc<FakeState>) -> Self {
        state
            .active_screenshot_streams
            .fetch_add(1, Ordering::SeqCst);
        let mut stopped = state.stopped.subscribe();
        Self {
            inner,
            state,
            stopped: Box::pin(async move {
                let _ = stopped.wait_for(|stopped| *stopped).await;
            }),
            ended: false,
        }
    }
}

impl Stream for CountedStream {
    type Item = Result<Image, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.ended {
            return Poll::Ready(None);
        }
        if self.stopped.as_mut().poll(cx).is_ready() {
            self.ended = true;
            return Poll::Ready(Some(Err(Status::unavailable("the emulator exited"))));
        }
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for CountedStream {
    fn drop(&mut self) {
        self.state
            .active_screenshot_streams
            .fetch_sub(1, Ordering::SeqCst);
    }
}

struct FakeService {
    state: Arc<FakeState>,
}

#[tonic::async_trait]
impl EmulatorController for FakeService {
    async fn get_vm_state(&self, _request: Request<()>) -> Result<Response<VmRunState>, Status> {
        self.state
            .record(|recorded| recorded.get_vm_state_calls += 1);
        Ok(Response::new(VmRunState {
            state: RunState::Running as i32,
        }))
    }

    async fn set_vm_state(&self, request: Request<VmRunState>) -> Result<Response<()>, Status> {
        let state = request.into_inner();
        self.state
            .record(|recorded| recorded.vm_states_set.push(state));
        Ok(Response::new(()))
    }

    async fn send_key(&self, request: Request<KeyboardEvent>) -> Result<Response<()>, Status> {
        let key = request.into_inner();
        self.state.record(|recorded| recorded.keys.push(key));
        Ok(Response::new(()))
    }

    async fn set_physical_model(
        &self,
        request: Request<PhysicalModelValue>,
    ) -> Result<Response<()>, Status> {
        let value = request.into_inner();
        self.state
            .record(|recorded| recorded.physical_models.push(value));
        Ok(Response::new(()))
    }

    async fn stream_input_event(
        &self,
        request: Request<Streaming<InputEvent>>,
    ) -> Result<Response<()>, Status> {
        let mut events = request.into_inner();
        while let Some(event) = events.message().await? {
            self.state
                .record(|recorded| recorded.input_events.push(event));
        }
        Ok(Response::new(()))
    }

    async fn get_screenshot(
        &self,
        _request: Request<ImageFormat>,
    ) -> Result<Response<Image>, Status> {
        Ok(Response::new(Image {
            image: Bytes::from_static(FAKE_PNG),
            ..Default::default()
        }))
    }

    async fn stream_screenshot(
        &self,
        request: Request<ImageFormat>,
    ) -> Result<Response<BoxStream<Image>>, Status> {
        let format = request.into_inner();
        if format.format != ImgFormat::Rgba8888 as i32 {
            return Err(Status::unimplemented("the fake only streams RGBA8888"));
        }
        let config = &self.state.config;
        let (width, height) =
            fake_frame_size(config.display, config.rotation, format.width, format.height);
        let rotation = config.rotation;
        let frame_interval = config.frame_interval;
        let frames = tokio_stream::iter(1..=config.frames_per_stream)
            .then(move |seq| async move {
                if let Some(interval) = frame_interval.filter(|_| seq > 1) {
                    tokio::time::sleep(interval).await;
                }
                Ok(fake_frame(width, height, rotation, seq))
            })
            .chain(tokio_stream::pending());
        Ok(Response::new(Box::pin(CountedStream::new(
            Box::pin(frames),
            self.state.clone(),
        ))))
    }
}
