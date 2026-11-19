//! Warpium: the setting and command-palette toggle for pasting images into containers and SSH
//! sessions, and running a paste off the UI thread. The terminal's side is
//! `terminal/view/warpium_container_paste.rs`; the logic is the `warpium_container_paste` crate.

use warpui::{AppContext, SingletonEntity as _};

use super::sync_settings::{CONTAINER_IMAGE_PASTE_CONTEXT_FLAG, WarpiumContainerPasteSettings};
use crate::settings_view::ToggleSettingActionPair;
use crate::workspace::WorkspaceAction;

/// Whether `warpium.container_image_paste` is on in a Warpium build.
pub(crate) fn is_enabled(ctx: &AppContext) -> bool {
    #[cfg(target_family = "wasm")]
    {
        let _ = ctx;
        false
    }
    #[cfg(not(target_family = "wasm"))]
    {
        super::is_warpium_channel()
            && ctx.has_singleton_model::<WarpiumContainerPasteSettings>()
            && *WarpiumContainerPasteSettings::as_ref(ctx).container_image_paste
    }
}

/// The container set in `warpium.container_image_paste_container`, if any.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn override_container(ctx: &AppContext) -> Option<String> {
    let value = WarpiumContainerPasteSettings::as_ref(ctx)
        .container_image_paste_container
        .trim()
        .to_owned();
    (!value.is_empty()).then_some(value)
}

pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "image paste into containers and SSH sessions",
            WorkspaceAction::ToggleWarpiumContainerImagePaste,
            &id!("Workspace"),
            CONTAINER_IMAGE_PASTE_CONTEXT_FLAG,
        )],
        app,
    );
}

/// Runs [`warpium_container_paste::paste`] on its own thread.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn spawn_paste(
    request: warpium_container_paste::PasteRequest,
) -> futures::channel::oneshot::Receiver<
    Result<warpium_container_paste::Pasted, warpium_container_paste::PasteError>,
> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("warpium-container-paste".to_owned())
        .spawn(move || {
            let _ = sender.send(warpium_container_paste::paste(&request));
        });
    if let Err(err) = spawned {
        log::error!("[Container paste] Couldn't start the paste thread: {err}");
    }
    receiver
}
