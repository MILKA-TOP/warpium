//! Warpium: hands images pasted or dropped onto a terminal to the CLI agent running in its
//! container or SSH session. Agents there can't read the Mac's clipboard, so the image is copied
//! in with the host's `docker`/`podman`/`ssh` and its path there is pasted instead.

use std::path::{Path, PathBuf};

use warpium_container_paste::{
    DEFAULT_REMOTE_DIR, ImageSource, OwnedHints, PasteImage, PasteRequest, Pasted, Target, Tools,
    choose_clipboard_image, parse_command,
};
use warpui::clipboard::ClipboardContent;
use warpui::clipboard_utils::has_image_extension;
use warpui::{AppContext, ViewContext};

use super::{BRACKETED_PASTE_PREFIX, BRACKETED_PASTE_SUFFIX};
use crate::terminal::TerminalView;
use crate::terminal::model::block::BlockId;

impl TerminalView {
    /// The container or SSH host the active long-running command runs in, with what is known
    /// about where it was started. `None` while the setting is off or nothing is running.
    fn warpium_container_paste_target(&self, ctx: &AppContext) -> Option<(Target, OwnedHints)> {
        if !crate::warpium::container_paste::is_enabled(ctx) {
            return None;
        }
        let (command, pwd) = {
            let model = self.model.lock();
            let block = model.block_list().active_block();
            if !block.is_active_and_long_running() {
                return None;
            }
            (
                block.command_with_secrets_unobfuscated(false),
                block.pwd().cloned(),
            )
        };
        let session = self
            .active_block_session_id()
            .and_then(|session_id| self.sessions.as_ref(ctx).get(session_id));
        let resolve_alias = |word: &str| {
            session
                .as_ref()
                .and_then(|session| session.aliases().get(word).cloned())
        };

        // Commands typed in a shell on another machine name containers and hosts the Mac's CLIs
        // can't reach.
        let is_host_shell = session
            .as_ref()
            .is_none_or(|session| session.is_local() && !session.is_subshell_or_ssh());

        // A command typed in the host's shell: `docker compose run app zsh`, `ssh box`.
        if is_host_shell && let Some(target) = parse_command(&command, resolve_alias) {
            return Some((
                target,
                OwnedHints {
                    cwd: session.as_ref().and(pwd).map(PathBuf::from),
                    hostname: None,
                },
            ));
        }

        // A shell Warp tracks inside the container or on the host (a warpified subshell), where
        // the agent itself is the running command.
        if let Some(session) = &session
            && let Some(info) = session.subshell_info()
            && let Some(target) = parse_command(&info.spawning_command, |_| None)
        {
            return Some((
                target,
                OwnedHints {
                    cwd: None,
                    hostname: Some(session.hostname().to_owned()),
                },
            ));
        }

        // Agents Warp detects on the host read the clipboard themselves.
        if !is_host_shell || self.has_active_cli_agent_session(ctx) {
            return None;
        }
        let target = crate::warpium::container_paste::override_container(ctx)
            .and_then(|container| Target::from_override(&container))?;
        Some((target, OwnedHints::default()))
    }

    /// Handles a paste with an image on the clipboard (or only image files) while the terminal
    /// runs a container or SSH session. Returns whether it took the paste.
    pub(super) fn warpium_paste_image_into_container(
        &mut self,
        content: &ClipboardContent,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let images = match &content.paths {
            Some(paths) if !paths.is_empty() => {
                if !paths.iter().all(|path| has_image_extension(path)) {
                    return false;
                }
                paths.iter().map(|path| image_from_file(path)).collect()
            }
            _ => {
                if !content.has_image_data() || !content.plain_text.trim().is_empty() {
                    return false;
                }
                let images = content.images.as_deref().unwrap_or_default();
                let mime_types = images
                    .iter()
                    .map(|image| image.mime_type.as_str())
                    .collect::<Vec<_>>();
                let Some((index, extension)) = choose_clipboard_image(&mime_types) else {
                    return false;
                };
                let image = &images[index];
                vec![PasteImage {
                    source: ImageSource::Bytes(image.data.clone()),
                    extension: extension.to_owned(),
                    original_name: image.filename.clone(),
                }]
            }
        };
        self.warpium_paste_images_into_container(images, ctx)
    }

    /// Handles image files dropped on the terminal while it runs a container or SSH session.
    /// Returns whether it took the drop.
    pub(super) fn warpium_drop_images_into_container(
        &mut self,
        paths: &[String],
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if paths.is_empty() || !paths.iter().all(|path| has_image_extension(path)) {
            return false;
        }
        let images = paths.iter().map(|path| image_from_file(path)).collect();
        self.warpium_paste_images_into_container(images, ctx)
    }

    fn warpium_paste_images_into_container(
        &mut self,
        images: Vec<PasteImage>,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some((target, hints)) = self.warpium_container_paste_target(ctx) else {
            return false;
        };
        let block_id = self.model.lock().block_list().active_block_id().clone();
        let request = PasteRequest {
            target: target.clone(),
            hints,
            images,
            remote_dir: DEFAULT_REMOTE_DIR.to_owned(),
            tools: Tools::from_env(),
        };
        let receiver = crate::warpium::container_paste::spawn_paste(request);
        ctx.spawn(
            async move { receiver.await.ok() },
            move |me, result, ctx| {
                let Some(result) = result else {
                    return;
                };
                match result {
                    Ok(pasted) => me.warpium_type_pasted_paths(&target, &pasted, &block_id, ctx),
                    Err(err) => {
                        log::warn!("[Container paste] Pasting an image failed: {}", err.kind());
                        me.show_error_toast(format!("Couldn't paste the image: {err}"), ctx);
                    }
                }
            },
        );
        true
    }

    /// Pastes the copied images' paths into the session that asked for them, if it is still
    /// running.
    fn warpium_type_pasted_paths(
        &mut self,
        target: &Target,
        pasted: &Pasted,
        block_id: &BlockId,
        ctx: &mut ViewContext<Self>,
    ) {
        let needs_bracketed_paste = {
            let mut model = self.model.lock();
            let block_list = model.block_list();
            if block_list.active_block_id() != block_id
                || !block_list.active_block().is_active_and_long_running()
            {
                return;
            }
            model.needs_bracketed_paste()
        };
        log::info!(
            "[Container paste] Pasted {} image(s) ({:?})",
            pasted.paths.len(),
            pasted.method
        );
        // One paste per path: Codex only attaches a pasted path that is the whole paste.
        let text = pasted
            .paths
            .iter()
            .map(|path| {
                let text = target.paste_text_for_path(path);
                if needs_bracketed_paste {
                    format!("{BRACKETED_PASTE_PREFIX}{text}{BRACKETED_PASTE_SUFFIX}")
                } else {
                    text
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        self.write_user_bytes_to_pty(format!("{text} ").into_bytes(), ctx);
    }
}

fn image_from_file(path: &str) -> PasteImage {
    let path = Path::new(path);
    PasteImage {
        source: ImageSource::File(path.to_path_buf()),
        extension: path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_default(),
        original_name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
    }
}
