//! Warpium-only features. Kept in their own module so upstream merges stay clean.

#[cfg(not(target_family = "wasm"))]
pub(crate) mod android;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod code_index;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod code_index_palette;
pub(crate) mod code_navigation;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod logcat;
pub(crate) mod project_root;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod project_toolbar;
#[cfg(not(target_family = "wasm"))]
mod project_toolbar_row;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod self_update;
pub(crate) mod sync_settings;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod upstream_check;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod warp_import;

#[cfg(not(target_family = "wasm"))]
use std::sync::Arc;
#[cfg(not(target_family = "wasm"))]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(not(target_family = "wasm"))]
use warpium_upstream_check::{HttpRequest, HttpResponse};
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

/// Sent with every request to GitHub, which requires a `User-Agent`.
#[cfg(not(target_family = "wasm"))]
pub(crate) const GITHUB_USER_AGENT: &str = "Warpium";
#[cfg(not(target_family = "wasm"))]
const GITHUB_API_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(not(target_family = "wasm"))]
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Sends an unauthenticated GitHub REST API request and returns the parts the Warpium checks
/// need, or a transport error message.
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn fetch_github(
    client: Arc<http_client::Client>,
    request: HttpRequest,
) -> Result<HttpResponse, String> {
    let mut builder = client
        .get(request.url.as_str())
        // GitHub's REST API requires a User-Agent and recommends pinning the media type and
        // version: https://docs.github.com/en/rest/using-the-rest-api/getting-started-with-the-rest-api
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", GITHUB_USER_AGENT)
        .timeout(GITHUB_API_TIMEOUT);
    if let Some(etag) = &request.if_none_match {
        builder = builder.header("If-None-Match", etag.as_str());
    }
    let response = builder.send().await.map_err(|err| format!("{err:#}"))?;
    let status = response.status().as_u16();
    let [etag, retry_after, rate_limit_remaining, rate_limit_reset] = [
        "etag",
        "retry-after",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
    ]
    .map(|name| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    });
    let body = response
        .bytes()
        .await
        .map_err(|err| format!("{err:#}"))?
        .to_vec();
    Ok(HttpResponse {
        status,
        etag,
        retry_after,
        rate_limit_remaining,
        rate_limit_reset,
        body,
    })
}

/// Whether this is a Warpium build: Warpium is Warp's OSS channel.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn is_warpium_channel() -> bool {
    use crate::channel::{Channel, ChannelState};

    matches!(ChannelState::channel(), Channel::Oss)
}
