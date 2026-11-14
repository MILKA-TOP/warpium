//! Warpium-only features. Kept in their own module so upstream merges stay clean.

#[cfg(not(target_family = "wasm"))]
pub(crate) mod self_update;
pub(crate) mod sync_settings;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod upstream_check;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod warp_import;

#[cfg(not(target_family = "wasm"))]
use warpui::{Entity, ModelContext, SingletonEntity as _};

#[cfg(not(target_family = "wasm"))]
use crate::view_components::DismissibleToast;
#[cfg(not(target_family = "wasm"))]
use crate::workspace::{ToastStack, WorkspaceAction};

/// Shows an ephemeral toast in the active window, or else the first open one.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn show_toast<M: Entity>(
    toast: DismissibleToast<WorkspaceAction>,
    ctx: &mut ModelContext<M>,
) {
    let window_id = ctx
        .windows()
        .active_window()
        .or_else(|| ctx.windows().ordered_window_ids().first().copied());
    let Some(window_id) = window_id else {
        return;
    };
    ToastStack::handle(ctx).update(ctx, |stack, ctx| {
        stack.add_ephemeral_toast(toast, window_id, ctx);
    });
}
