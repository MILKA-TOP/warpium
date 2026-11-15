//! Go to Class / Go to Symbol for Kotlin and Java projects, backed by the in-process index in
//! `crates/code_index`.
//!
//! One project is indexed at a time: the pinned Project Explorer root or the repository of the
//! active session, picked when a Go to palette opens. The index is built (or loaded from its
//! snapshot in the user cache directory and rescanned) on a dedicated thread, kept current from
//! the `repo_metadata` file watcher, and searched by the palette off the main thread. The main
//! thread only ever swaps handles; it never parses, walks or locks the index for long.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use instant::Instant;

use code_index::{CodeIndex, PersistError};
use futures::channel::oneshot;
use parking_lot::RwLock;
use repo_metadata::repository::{RepositorySubscriber, SubscriberId};
use repo_metadata::{DirectoryWatcher, Repository, RepositoryUpdate, RepositoryWatchMode};
use warp_util::standardized_path::StandardizedPath;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::keymap::EditableBinding;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, WeakModelHandle};

use super::is_warpium_channel;
use super::sync_settings::{CODE_INDEX_CONTEXT_FLAG, WarpiumSettings, WarpiumSettingsChangedEvent};
use crate::palette::PaletteMode;
use crate::server::telemetry::PaletteSource;
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::workspace::WorkspaceAction;

/// How long after the last file change the snapshot is rewritten.
const SAVE_DELAY: Duration = Duration::from_secs(10);
/// Worker threads for incremental updates; a save touches one or a few files.
const UPDATE_THREADS: usize = 2;

/// Whether the project's index is still being built or brought up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndexState {
    Indexing,
    Ready,
}

/// What the palette needs to search: a shared handle to the index of the current project.
#[derive(Clone)]
pub(crate) struct IndexHandle {
    pub root: PathBuf,
    pub index: Arc<RwLock<CodeIndex>>,
    pub state: IndexState,
}

pub(crate) struct CodeIndexModel {
    project: Option<Project>,
    /// Bumped for every new project, so results of work started for an older one are dropped.
    generation: u64,
}

struct Project {
    root: PathBuf,
    snapshot: Option<PathBuf>,
    index: Arc<RwLock<CodeIndex>>,
    state: IndexState,
    watch: Option<(WeakModelHandle<Repository>, SubscriberId)>,
    /// Changed paths waiting for the running update to finish.
    pending: HashSet<PathBuf>,
    update_running: bool,
    save: Option<SpawnedFutureHandle>,
}

impl Entity for CodeIndexModel {
    type Event = ();
}

impl SingletonEntity for CodeIndexModel {}

pub(crate) fn init(ctx: &mut AppContext) {
    ctx.add_singleton_model(|ctx: &mut ModelContext<CodeIndexModel>| {
        ctx.subscribe_to_model(
            &WarpiumSettings::handle(ctx),
            |model, _, event, ctx| match event {
                WarpiumSettingsChangedEvent::CodeIndex { .. } => {
                    if !is_enabled(ctx) {
                        model.close_project(ctx);
                    }
                }
                WarpiumSettingsChangedEvent::SyncFromWarp { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheck { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckIntervalHours { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckMinCommits { .. }
                | WarpiumSettingsChangedEvent::UpdateCheck { .. }
                | WarpiumSettingsChangedEvent::UpdateCheckIntervalHours { .. } => {}
            },
        );
        CodeIndexModel {
            project: None,
            generation: 0,
        }
    });
}

/// Registers the Go to Class / Go to Symbol palettes and the setting's palette toggle.
pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    // JetBrains uses cmd-O and cmd-alt-O, but Warp already binds them (Files palette and
    // /open-repo), so these add ctrl. Both are editable.
    app.register_editable_bindings([
        EditableBinding::new(
            "workspace:go_to_class",
            "Go to Class",
            WorkspaceAction::TogglePalette {
                mode: PaletteMode::CodeClasses,
                source: PaletteSource::Keybinding,
            },
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(id!("Workspace") & id!(CODE_INDEX_CONTEXT_FLAG))
        .with_mac_key_binding("ctrl-cmd-o"),
        EditableBinding::new(
            "workspace:go_to_symbol",
            "Go to Symbol",
            WorkspaceAction::TogglePalette {
                mode: PaletteMode::CodeSymbols,
                source: PaletteSource::Keybinding,
            },
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(id!("Workspace") & id!(CODE_INDEX_CONTEXT_FLAG))
        .with_mac_key_binding("ctrl-alt-cmd-o"),
    ]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "Kotlin/Java code index (Go to Class/Symbol)",
            WorkspaceAction::ToggleCodeIndex,
            &id!("Workspace"),
            CODE_INDEX_CONTEXT_FLAG,
        )],
        app,
    );
}

/// Whether Go to Class/Symbol are available: Warpium builds with `warpium.code_index` on.
pub(crate) fn is_enabled(ctx: &AppContext) -> bool {
    is_warpium_channel() && *WarpiumSettings::as_ref(ctx).code_index
}

/// Starts indexing `root` unless it is already the current project. Without a root (no local
/// directory in the active tab) the current project stays.
pub(crate) fn ensure_project(root: Option<PathBuf>, ctx: &mut AppContext) {
    let Some(root) = root else {
        return;
    };
    if !ctx.has_singleton_model::<CodeIndexModel>() {
        return;
    }
    CodeIndexModel::handle(ctx).update(ctx, |model, ctx| model.open_project(root, ctx));
}

impl CodeIndexModel {
    /// The current project's index, if there is one.
    pub(crate) fn handle_for_search(&self) -> Option<IndexHandle> {
        self.project.as_ref().map(|project| IndexHandle {
            root: project.root.clone(),
            index: project.index.clone(),
            state: project.state,
        })
    }

    fn open_project(&mut self, root: PathBuf, ctx: &mut ModelContext<Self>) {
        if !is_enabled(ctx) {
            return;
        }
        // Watcher paths are canonical, so the root must be too for prefix checks.
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        if self
            .project
            .as_ref()
            .is_some_and(|project| project.root == root)
        {
            return;
        }
        self.close_project(ctx);
        self.generation += 1;

        let mut project = Project {
            snapshot: code_index::default_snapshot_path(&root),
            index: Arc::new(RwLock::new(CodeIndex::new(root.clone()))),
            root,
            state: IndexState::Indexing,
            watch: None,
            pending: HashSet::new(),
            update_running: false,
            save: None,
        };
        project.watch = self.watch(&project.root, ctx);
        self.start_build(&project, ctx);
        self.project = Some(project);
        ctx.notify();
    }

    fn close_project(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(project) = self.project.take() else {
            return;
        };
        self.generation += 1;
        if let Some(save) = project.save {
            save.abort();
        }
        if let Some((repository, subscriber_id)) = project.watch
            && let Some(repository) = repository.upgrade(ctx)
        {
            repository.update(ctx, |repository, ctx| {
                repository.stop_watching(subscriber_id, ctx);
            });
        }
        // The index is freed once in-flight searches drop their handles.
        ctx.notify();
    }

    /// Loads the snapshot, rescans the root and saves the snapshot again, on its own thread.
    fn start_build(&self, project: &Project, ctx: &mut ModelContext<Self>) {
        let (sender, receiver) = oneshot::channel();
        let root = project.root.clone();
        let snapshot = project.snapshot.clone();
        let index = project.index.clone();
        let spawned = std::thread::Builder::new()
            .name("code-index-build".to_owned())
            .spawn(move || {
                let report = build(&root, snapshot.as_deref(), &index);
                let _ = sender.send(report);
            });
        if let Err(err) = spawned {
            log::error!("[Code index] Couldn't start the indexing thread: {err}");
            return;
        }
        let generation = self.generation;
        ctx.spawn(
            async move { receiver.await.ok() },
            move |model, report, ctx| {
                if model.generation != generation {
                    return;
                }
                if let Some(report) = report {
                    log::info!(
                        "[Code index] Indexed {} files, {} symbols in {} ms (snapshot loaded in \
                         {:?} ms, {} files parsed)",
                        report.files,
                        report.symbols,
                        report.total.as_millis(),
                        report.loaded_in.map(|loaded| loaded.as_millis()),
                        report.parsed
                    );
                }
                if let Some(project) = &mut model.project {
                    project.state = IndexState::Ready;
                }
                model.run_update(ctx);
                ctx.notify();
            },
        );
    }

    fn watch(
        &self,
        root: &Path,
        ctx: &mut ModelContext<Self>,
    ) -> Option<(WeakModelHandle<Repository>, SubscriberId)> {
        let standardized = StandardizedPath::from_local_canonicalized(root).ok()?;
        let repository = DirectoryWatcher::handle(ctx)
            .update(ctx, |watcher, ctx| watcher.add_directory(standardized, ctx))
            .inspect_err(|err| log::warn!("[Code index] Couldn't watch the project: {err}"))
            .ok()?;
        let (sender, receiver) = async_channel::unbounded::<RepositoryUpdate>();
        let start = repository.update(ctx, |repository, ctx| {
            repository.start_watching(
                RepositoryWatchMode::FilesystemOnly,
                Box::new(Subscriber { sender }),
                ctx,
            )
        });
        let subscriber_id = start.subscriber_id;
        ctx.spawn(start.registration_future, |_, result, _| {
            if let Err(err) = result {
                log::warn!("[Code index] Couldn't watch the project: {err}");
            }
        });
        let generation = self.generation;
        ctx.spawn_stream_local(
            receiver,
            move |model, update, ctx| {
                if model.generation == generation {
                    model.on_files_changed(update, ctx);
                }
            },
            |_, _| {},
        );
        Some((repository.downgrade(), subscriber_id))
    }

    fn on_files_changed(&mut self, update: RepositoryUpdate, ctx: &mut ModelContext<Self>) {
        let Some(project) = &mut self.project else {
            return;
        };
        let RepositoryUpdate {
            added,
            modified,
            deleted,
            moved,
            ..
        } = update;
        let changed = added
            .into_iter()
            .chain(modified)
            .chain(deleted)
            .chain(moved.into_iter().flat_map(|(to, from)| [to, from]))
            // Generated sources are usually gitignored but indexed.
            .filter(|file| !file.is_ignored || is_generated_source(&file.path))
            .map(|file| file.path)
            .filter(|path| path.starts_with(&project.root) && may_hold_sources(path));
        project.pending.extend(changed);
        if project.state == IndexState::Ready {
            self.run_update(ctx);
        }
    }

    /// Reindexes the pending paths on a worker thread, one batch at a time.
    fn run_update(&mut self, ctx: &mut ModelContext<Self>) {
        let generation = self.generation;
        let Some(project) = &mut self.project else {
            return;
        };
        if project.update_running || project.pending.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = project.pending.drain().collect();
        project.update_running = true;
        let root = project.root.clone();
        let index = project.index.clone();
        let (sender, receiver) = oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("code-index-update".to_owned())
            .spawn(move || {
                let started = Instant::now();
                // Parse without the lock; applying takes it only briefly.
                let update = code_index::prepare_changes(&root, &paths, UPDATE_THREADS);
                let stats = index.write().apply(update);
                let _ = sender.send((stats, started.elapsed()));
            });
        if let Err(err) = spawned {
            log::error!("[Code index] Couldn't start the update thread: {err}");
            project.update_running = false;
            return;
        }
        ctx.spawn(
            async move { receiver.await.ok() },
            move |model, result, ctx| {
                if model.generation != generation {
                    return;
                }
                if let Some((stats, elapsed)) = result {
                    log::debug!(
                        "[Code index] Updated {} files, removed {} in {} ms",
                        stats.files_parsed,
                        stats.files_removed,
                        elapsed.as_millis()
                    );
                }
                if let Some(project) = &mut model.project {
                    project.update_running = false;
                }
                model.schedule_save(ctx);
                model.run_update(ctx);
            },
        );
    }

    /// Rewrites the snapshot `SAVE_DELAY` after the last change.
    fn schedule_save(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(project) = &mut self.project else {
            return;
        };
        let Some(snapshot) = project.snapshot.clone() else {
            return;
        };
        if let Some(save) = project.save.take() {
            save.abort();
        }
        let index = project.index.clone();
        project.save = Some(ctx.spawn(
            async move {
                Timer::after(SAVE_DELAY).await;
                save_snapshot(&index, &snapshot);
            },
            |_, _, _| {},
        ));
    }
}

struct BuildReport {
    files: usize,
    symbols: usize,
    parsed: usize,
    loaded_in: Option<Duration>,
    total: Duration,
}

/// Loads the snapshot into `index` (so the palette can search right away), rescans the root in
/// batches and saves the snapshot if anything changed.
fn build(root: &Path, snapshot: Option<&Path>, index: &RwLock<CodeIndex>) -> BuildReport {
    let started = Instant::now();
    let mut loaded_in = None;
    if let Some(snapshot) = snapshot {
        match CodeIndex::load(snapshot, root) {
            Ok(loaded) => {
                *index.write() = loaded;
                loaded_in = Some(started.elapsed());
            }
            Err(PersistError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => log::warn!("[Code index] Rebuilding the index: {err}"),
        }
    }
    let stamps = index.read().stamps();
    let mut changed = 0;
    code_index::refresh_in_batches(root, &stamps, code_index::default_threads(), |batch| {
        let stats = index.write().apply(batch);
        changed += stats.files_parsed + stats.files_removed;
    });
    if changed > 0
        && let Some(snapshot) = snapshot
    {
        save_snapshot(index, snapshot);
    }
    let stats = index.read().stats();
    BuildReport {
        files: stats.files,
        symbols: stats.symbols,
        parsed: changed,
        loaded_in,
        total: started.elapsed(),
    }
}

fn save_snapshot(index: &RwLock<CodeIndex>, snapshot: &Path) {
    if let Err(err) = index.read().save(snapshot) {
        log::warn!("[Code index] Couldn't save the index snapshot: {err}");
    }
}

fn is_generated_source(path: &Path) -> bool {
    let components: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
    components
        .windows(2)
        .any(|pair| pair[0] == "build" && pair[1] == "generated")
}

/// Kotlin and Java files, and extensionless paths that may be (deleted or moved) directories.
fn may_hold_sources(path: &Path) -> bool {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("kt" | "java") => true,
        Some(_) => false,
        None => !path.is_file(),
    }
}

struct Subscriber {
    sender: async_channel::Sender<RepositoryUpdate>,
}

impl RepositorySubscriber for Subscriber {
    fn on_scan(
        &mut self,
        _repository: &Repository,
        _ctx: &mut ModelContext<Repository>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        Box::pin(async {})
    }

    fn on_files_updated(
        &mut self,
        _repository: &Repository,
        update: &RepositoryUpdate,
        _ctx: &mut ModelContext<Repository>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        let sender = self.sender.clone();
        let update = update.clone();
        Box::pin(async move {
            let _ = sender.send(update).await;
        })
    }
}
