//! Live device list: follows `host:track-devices` and reads the details of each device that comes
//! online.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crate::adb::{AdbClient, AdbError};
use crate::device::{Device, DeviceEntry, DeviceProps, read_props};
use crate::worker::{WorkerControl, WorkerHandle};

const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackerEvent {
    /// The full device list, sent on every change.
    Devices(Vec<Device>),
    /// The server couldn't be reached; the tracker retries with a growing delay.
    Unavailable(String),
}

/// Starts tracking devices on a background thread. `sink` is called on that thread; dropping the
/// handle stops it.
pub fn spawn_device_tracker(
    client: AdbClient,
    mut sink: impl FnMut(TrackerEvent) + Send + 'static,
) -> io::Result<WorkerHandle> {
    WorkerHandle::spawn("adb-track-devices", move |control| {
        let mut props_cache = PropsCache::default();
        let mut retry_delay = Duration::from_secs(1);
        while !control.is_stopped() {
            let result = track(
                &client,
                &control,
                &mut props_cache,
                &mut sink,
                &mut retry_delay,
            );
            if control.is_stopped() {
                return;
            }
            if let Err(err) = result {
                log::debug!("[Android] Device tracking stopped: {err}");
                sink(TrackerEvent::Unavailable(err.to_string()));
            }
            if !control.sleep(retry_delay) {
                return;
            }
            retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
        }
    })
}

fn track(
    client: &AdbClient,
    control: &WorkerControl,
    props_cache: &mut PropsCache,
    sink: &mut impl FnMut(TrackerEvent),
    retry_delay: &mut Duration,
) -> Result<(), AdbError> {
    let mut tracking = client.track_devices()?;
    if !control.set_socket(&tracking.shutdown_handle()?) {
        return Ok(());
    }
    *retry_delay = Duration::from_secs(1);
    loop {
        let entries = tracking.next_list()?;
        let devices = props_cache.enrich(client, entries);
        sink(TrackerEvent::Devices(devices));
    }
}

/// Device details by serial and transport, so a reconnected device is read again.
#[derive(Default)]
struct PropsCache {
    props: HashMap<(String, Option<u64>), DeviceProps>,
}

impl PropsCache {
    fn enrich(&mut self, client: &AdbClient, entries: Vec<DeviceEntry>) -> Vec<Device> {
        self.props.retain(|(serial, transport_id), _| {
            entries
                .iter()
                .any(|entry| entry.serial == *serial && entry.transport_id == *transport_id)
        });
        entries
            .into_iter()
            .map(|entry| {
                let key = (entry.serial.clone(), entry.transport_id);
                let props = if entry.state.is_online() {
                    match self.props.get(&key) {
                        Some(props) => Some(props.clone()),
                        None => match read_props(client, &entry.serial) {
                            // A device that has just come online may not answer yet; it is read
                            // again on the next change.
                            Ok(props) if props.api_level.is_some() => {
                                self.props.insert(key, props.clone());
                                Some(props)
                            }
                            Ok(_) => None,
                            Err(err) => {
                                log::debug!("[Android] Couldn't read device properties: {err}");
                                None
                            }
                        },
                    }
                } else {
                    None
                };
                Device { entry, props }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "tracker_tests.rs"]
mod tests;
