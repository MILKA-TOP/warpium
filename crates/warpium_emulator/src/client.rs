//! A client for the emulator's `EmulatorController` gRPC service.

use std::time::Duration;

use bytes::Bytes;
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    Request, Status, Streaming,
    metadata::{AsciiMetadataValue, MetadataValue},
    service::{Interceptor, interceptor::InterceptedService},
    transport::{Channel, Endpoint},
};

use crate::{
    discovery::{GrpcToken, RunningEmulator},
    frame_slot::{Frame, FrameSlot},
    geometry::{DisplayPoint, DisplayRotation},
    keys::DeviceButton,
    proto::{
        Image, ImageFormat, InputEvent, KeyboardEvent, MouseEvent, ParameterValue,
        PhysicalModelValue, VmRunState, WheelEvent, emulator_controller_client,
        image_format::ImgFormat, input_event, keyboard_event::KeyEventType,
        physical_model_value::PhysicalType, vm_run_state::RunState,
    },
};

/// How often to call `getVmState` so an emulator started with `-idle-grpc-timeout` stays up
/// (Android Studio uses the same period).
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(120);

/// Input events buffered before new ones are rejected; the stream drains far faster than a user
/// produces them.
pub const INPUT_QUEUE_CAPACITY: usize = 256;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MESSAGE_SIZE_HEADROOM: usize = 64 * 1024;
const PRIMARY_BUTTON: i32 = 1;

/// The largest gRPC message needed for an RGBA8888 frame of this size. tonic's 4 MB default is
/// below a single 1080×2400 frame.
pub const fn frame_message_limit(width: u32, height: u32) -> usize {
    width as usize * height as usize * 4 + MESSAGE_SIZE_HEADROOM
}

/// Fits any display up to 4096×4096, which covers phones, tablets and foldables.
pub const DEFAULT_MAX_DECODING_MESSAGE_SIZE: usize = frame_message_limit(4096, 4096);

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("invalid emulator gRPC endpoint")]
    InvalidEndpoint(#[source] tonic::transport::Error),
    #[error("the emulator gRPC token is not a valid header value")]
    InvalidToken,
    #[error("the emulator gRPC endpoint requires TLS, which is not supported")]
    TlsRequired,
    #[error("failed to connect to the emulator gRPC endpoint")]
    Connect(#[source] tonic::transport::Error),
    #[error("emulator gRPC call failed")]
    Rpc(#[from] Status),
    #[error("emulator sent a frame of {width}x{height} with {len} bytes")]
    MalformedFrame { width: u32, height: u32, len: usize },
    #[error("the input event queue is full")]
    InputQueueFull,
    #[error("the input event stream has closed")]
    InputClosed,
}

impl ClientError {
    /// The emulator is gone or unreachable, as opposed to rejecting a request.
    pub fn is_disconnect(&self) -> bool {
        match self {
            Self::Connect(_) | Self::InputClosed => true,
            Self::Rpc(status) => matches!(
                status.code(),
                tonic::Code::Unavailable | tonic::Code::Cancelled | tonic::Code::Unknown
            ),
            Self::InvalidEndpoint(_)
            | Self::InvalidToken
            | Self::TlsRequired
            | Self::MalformedFrame { .. }
            | Self::InputQueueFull => false,
        }
    }
}

/// Adds `authorization: Bearer <token>` to every call.
#[derive(Clone)]
struct BearerAuth {
    header: Option<AsciiMetadataValue>,
}

impl Interceptor for BearerAuth {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        if let Some(header) = &self.header {
            request
                .metadata_mut()
                .insert("authorization", header.clone());
        }
        Ok(request)
    }
}

type ControllerClient =
    emulator_controller_client::EmulatorControllerClient<InterceptedService<Channel, BearerAuth>>;

/// A connection to one emulator. Cloning shares the underlying HTTP/2 connection.
#[derive(Clone)]
pub struct EmulatorClient {
    inner: ControllerClient,
}

impl EmulatorClient {
    /// Connects to a discovered emulator with its token and the default message size limit.
    pub async fn connect(emulator: &RunningEmulator) -> Result<Self, ClientError> {
        if emulator.grpc_uses_tls {
            return Err(ClientError::TlsRequired);
        }
        Self::connect_to(
            emulator.grpc_endpoint(),
            emulator.grpc_token.as_ref(),
            DEFAULT_MAX_DECODING_MESSAGE_SIZE,
        )
        .await
    }

    pub async fn connect_to(
        endpoint: String,
        token: Option<&GrpcToken>,
        max_decoding_message_size: usize,
    ) -> Result<Self, ClientError> {
        let header = token
            .map(|token| {
                MetadataValue::try_from(format!("Bearer {}", token.as_str()))
                    .map_err(|_| ClientError::InvalidToken)
            })
            .transpose()?;
        let channel = Endpoint::from_shared(endpoint)
            .map_err(ClientError::InvalidEndpoint)?
            .connect_timeout(CONNECT_TIMEOUT)
            .connect()
            .await
            .map_err(ClientError::Connect)?;
        let inner = emulator_controller_client::EmulatorControllerClient::with_interceptor(
            channel,
            BearerAuth { header },
        )
        .max_decoding_message_size(max_decoding_message_size);
        Ok(Self { inner })
    }

    pub async fn vm_state(&mut self) -> Result<RunState, ClientError> {
        let state = self.inner.get_vm_state(()).await?.into_inner();
        Ok(RunState::try_from(state.state).unwrap_or(RunState::Unknown))
    }

    /// Resets the emulator's `-idle-grpc-timeout` with a cheap call.
    pub async fn keepalive(&mut self) -> Result<(), ClientError> {
        self.vm_state().await.map(|_| ())
    }

    /// Starts an RGBA8888 frame stream scaled by the emulator to fit `width`×`height` physical
    /// pixels (it keeps the aspect ratio and never exceeds the display). `0` means native size.
    pub async fn stream_screenshot(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<FrameStream, ClientError> {
        let request = ImageFormat {
            format: ImgFormat::Rgba8888 as i32,
            width,
            height,
            ..Default::default()
        };
        let inner = self.inner.stream_screenshot(request).await?.into_inner();
        Ok(FrameStream { inner })
    }

    /// A full-resolution PNG screenshot.
    pub async fn screenshot_png(&mut self) -> Result<Bytes, ClientError> {
        let request = ImageFormat {
            format: ImgFormat::Png as i32,
            ..Default::default()
        };
        Ok(self.inner.get_screenshot(request).await?.into_inner().image)
    }

    /// Presses and releases a key given by its W3C name (`"Enter"`, `"GoBack"`, `"a"`).
    pub async fn send_key(&mut self, key: &str) -> Result<(), ClientError> {
        let event = KeyboardEvent {
            event_type: KeyEventType::Keypress as i32,
            key: key.to_owned(),
            ..Default::default()
        };
        self.inner.send_key(event).await?;
        Ok(())
    }

    pub async fn press_button(&mut self, button: DeviceButton) -> Result<(), ClientError> {
        self.send_key(button.w3c_key()).await
    }

    pub async fn set_rotation(&mut self, rotation: DisplayRotation) -> Result<(), ClientError> {
        let value = PhysicalModelValue {
            target: PhysicalType::Rotation as i32,
            value: Some(ParameterValue {
                data: vec![0.0, 0.0, rotation.physical_angle()],
            }),
            ..Default::default()
        };
        self.inner.set_physical_model(value).await?;
        Ok(())
    }

    /// Asks the emulator to shut down gracefully.
    pub async fn shutdown(&mut self) -> Result<(), ClientError> {
        let state = VmRunState {
            state: RunState::Shutdown as i32,
        };
        self.inner.set_vm_state(state).await?;
        Ok(())
    }

    /// Opens the long-lived, ordered `streamInputEvent` call. Must be called within a tokio
    /// runtime, which drives the stream.
    pub fn open_input_stream(&self) -> InputSender {
        let (sender, receiver) = mpsc::channel(INPUT_QUEUE_CAPACITY);
        let mut inner = self.inner.clone();
        let task = tokio::spawn(async move {
            inner
                .stream_input_event(ReceiverStream::new(receiver))
                .await
                .map(|_| ())
        });
        InputSender { sender, task }
    }
}

/// Calls [`EmulatorClient::keepalive`] every `interval` until a call fails, and returns that
/// failure.
pub async fn run_keepalive(mut client: EmulatorClient, interval: Duration) -> ClientError {
    loop {
        tokio::time::sleep(interval).await;
        if let Err(err) = client.keepalive().await {
            log::debug!("[Emulator client] Keepalive failed: {err:#}");
            return err;
        }
        log::trace!("[Emulator client] Keepalive sent");
    }
}

/// Server-streamed frames. Dropping it cancels the call, which stops the emulator's readback.
pub struct FrameStream {
    inner: Streaming<Image>,
}

impl FrameStream {
    /// The next frame, or `None` when the emulator ended the stream.
    pub async fn next_frame(&mut self) -> Result<Option<Frame>, ClientError> {
        match self.inner.message().await? {
            Some(image) => frame_from_image(image).map(Some),
            None => Ok(None),
        }
    }
}

/// Moves frames into `slot` until the stream ends, calling `on_new_frame` when the consumer has
/// to be woken (see [`FrameSlot::put`]).
pub async fn pump_frames(
    mut stream: FrameStream,
    slot: &FrameSlot,
    mut on_new_frame: impl FnMut(),
) -> Result<(), ClientError> {
    while let Some(frame) = stream.next_frame().await? {
        if slot.put(frame) {
            on_new_frame();
        }
    }
    Ok(())
}

fn frame_from_image(image: Image) -> Result<Frame, ClientError> {
    let format = image.format.unwrap_or_default();
    // Older emulators fill only the top-level size fields, newer ones only `format`.
    #[allow(deprecated)]
    let (width, height) = if format.width != 0 || format.height != 0 {
        (format.width, format.height)
    } else {
        (image.width, image.height)
    };
    let pixels = image.image;
    let expected_len = width as usize * height as usize * 4;
    if pixels.len() != expected_len {
        return Err(ClientError::MalformedFrame {
            width,
            height,
            len: pixels.len(),
        });
    }
    let rotation = format
        .rotation
        .map(|rotation| DisplayRotation::from_proto(rotation.rotation))
        .unwrap_or_default();
    Ok(Frame {
        width,
        height,
        rotation,
        seq: image.seq,
        timestamp_us: image.timestamp_us,
        pixels,
    })
}

/// The sending half of `streamInputEvent`. Sends never block: events are queued, and rejected
/// when the queue is full.
pub struct InputSender {
    sender: mpsc::Sender<InputEvent>,
    task: JoinHandle<Result<(), Status>>,
}

impl InputSender {
    /// A single-finger mouse event; `pressed` is whether the primary button is down.
    pub fn send_mouse(&self, point: DisplayPoint, pressed: bool) -> Result<(), ClientError> {
        self.send(input_event::Type::MouseEvent(MouseEvent {
            x: point.x,
            y: point.y,
            buttons: if pressed { PRIMARY_BUTTON } else { 0 },
            display: 0,
        }))
    }

    /// Scroll by `dx`/`dy`, where 120 is one wheel notch.
    pub fn send_wheel(&self, dx: i32, dy: i32) -> Result<(), ClientError> {
        self.send(input_event::Type::WheelEvent(WheelEvent {
            dx,
            dy,
            ..Default::default()
        }))
    }

    pub fn send(&self, event: input_event::Type) -> Result<(), ClientError> {
        self.sender
            .try_send(InputEvent {
                r#type: Some(event),
            })
            .map_err(|err| match err {
                mpsc::error::TrySendError::Full(_) => ClientError::InputQueueFull,
                mpsc::error::TrySendError::Closed(_) => ClientError::InputClosed,
            })
    }

    /// Closes the stream after the queued events are delivered and waits for the call to end.
    pub async fn finish(self) -> Result<(), ClientError> {
        let Self { sender, task } = self;
        drop(sender);
        match task.await {
            Ok(result) => result.map_err(ClientError::from),
            Err(_) => Err(ClientError::InputClosed),
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
