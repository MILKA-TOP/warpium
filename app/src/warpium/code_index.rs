//! Go to Class / Go to Symbol, Go to Definition and Find Usages for Kotlin and Java projects,
//! backed by the in-process index in `crates/code_index`.
//!
//! One project is indexed at a time: the pinned Project Explorer root or the repository of the
//! active session, picked when a Go to palette opens. The index is built (or loaded from its
//! snapshot in the user cache directory and rescanned) on a dedicated thread, kept current from
//! the `repo_metadata` file watcher, and searched by the palette off the main thread. The main
//! thread only ever swaps handles; it never parses, walks or locks the index for long.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use code_index::{
    CodeIndex, Definition, Language, PersistError, Position, SourceFile, Usage, write_snapshot,
};
use futures::channel::oneshot;
use instant::Instant;
use parking_lot::RwLock;
use repo_metadata::repository::{RepositorySubscriber, SubscriberId};
use repo_metadata::{DirectoryWatcher, Repository, RepositoryUpdate, RepositoryWatchMode};
use warp_util::standardized_path::StandardizedPath;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::keymap::EditableBinding;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, WeakModelHandle};

use super::code_navigation::{
    CODE_NAVIGATION_CONTEXT_FLAG, CodeNavigationKind, CodeNavigationRequest,
};
use super::is_warpium_channel;
use super::sync_settings::{CODE_INDEX_CONTEXT_FLAG, WarpiumSettings, WarpiumSettingsChangedEvent};
use crate::code::local_code_editor::LocalCodeEditorAction;
use crate::palette::PaletteMode;
use crate::server::telemetry::PaletteSource;
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::workspace::WorkspaceAction;

/// How long after the last file change the snapshot is rewritten.
const SAVE_DELAY: Duration = Duration::from_secs(10);
/// Worker threads for incremental updates; a save touches one or a few files.
const UPDATE_THREADS: usize = 2;
/// Worker threads for scanning the files of a Find Usages query.
const USAGES_THREADS: usize = 4;
/// Most usages a Find Usages query lists.
const MAX_USAGES: usize = 500;

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
    /// What the navigation palette lists: the last Find Usages, or a Go to Definition with
    /// several candidates.
    navigation: Option<Arc<NavigationResults>>,
}

/// Results of a Go to Definition or Find Usages for the navigation palette.
#[derive(Debug)]
pub(crate) struct NavigationResults {
    pub root: PathBuf,
    /// What was looked up, as the palette's summary row says it.
    pub summary: String,
    pub items: Vec<NavigationItem>,
}

#[derive(Debug)]
pub(crate) enum NavigationItem {
    Definition(Definition),
    Usage(Usage),
}

/// What a Go to Definition or Find Usages request came to.
pub(crate) enum NavigationOutcome {
    /// One declaration: open it.
    Open { path: PathBuf, position: Position },
    /// Several declarations, or usages: list them in the navigation palette.
    Show(NavigationResults),
    /// Nothing to go to; tell the user why.
    Nothing(String),
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
    /// Set when the project is closed, so its build stops between batches.
    cancelled: Arc<AtomicBool>,
}

pub(crate) enum CodeIndexEvent {
    /// The current project finished indexing, so open Go to palettes can show full results.
    Ready,
}

impl Entity for CodeIndexModel {
    type Event = CodeIndexEvent;
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
                | WarpiumSettingsChangedEvent::UpdateCheckIntervalHours { .. }
                | WarpiumSettingsChangedEvent::ProjectToolbar { .. }
                | WarpiumSettingsChangedEvent::AndroidTools { .. } => {}
            },
        );
        CodeIndexModel {
            project: None,
            generation: 0,
            navigation: None,
        }
    });
}

/// Registers the Go to Class / Go to Symbol palettes and the setting's palette toggle.
pub(crate) fn init_bindings(ctx: &mut AppContext) {
    use warpui::keymap::macros::*;

    // JetBrains uses cmd-O and cmd-alt-O, but Warp already binds them (Files palette and
    // /open-repo), so these add ctrl. Both are editable.
    ctx.register_editable_bindings([
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

    // JetBrains keys. Warp's code editor binds neither cmd-B nor alt-F7.
    let in_navigable_editor =
        id!("LocalCodeEditorView") & id!(CODE_NAVIGATION_CONTEXT_FLAG) & !id!("IMEOpen");
    ctx.register_editable_bindings([
        EditableBinding::new(
            "editor:code_index_go_to_definition",
            "Go to Definition (Kotlin/Java code index)",
            LocalCodeEditorAction::CodeIndexNavigate(CodeNavigationKind::Definition),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(in_navigable_editor.clone())
        .with_mac_key_binding("cmd-b"),
        EditableBinding::new(
            "editor:code_index_find_usages",
            "Find Usages (Kotlin/Java code index)",
            LocalCodeEditorAction::CodeIndexNavigate(CodeNavigationKind::Usages),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(in_navigable_editor)
        .with_mac_key_binding("alt-f7"),
    ]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "Kotlin/Java code index (Go to Class/Symbol)",
            WorkspaceAction::ToggleCodeIndex,
            &id!("Workspace"),
            CODE_INDEX_CONTEXT_FLAG,
        )],
        ctx,
    );
}

/// Whether Go to Class/Symbol are available: Warpium builds with `warpium.code_index` on.
pub(crate) fn is_enabled(ctx: &AppContext) -> bool {
    is_warpium_channel() && *WarpiumSettings::as_ref(ctx).code_index
}

/// Starts indexing `root` unless it is already the current project. Without a usable root the
/// current project stays. The home directory and `/` are never indexed; a root that isn't a
/// pinned explorer root or repository (`require_build_files`) must contain Gradle or Maven build
/// files, so a stray working directory doesn't start a crawl.
pub(crate) fn ensure_project(
    root: Option<PathBuf>,
    require_build_files: bool,
    ctx: &mut AppContext,
) {
    let Some(root) = root else {
        return;
    };
    if !is_indexable_root(&root, require_build_files)
        || !ctx.has_singleton_model::<CodeIndexModel>()
    {
        return;
    }
    CodeIndexModel::handle(ctx).update(ctx, |model, ctx| model.open_project(root, ctx));
}

/// Makes sure the project holding `file` is indexed, when the current project doesn't hold it:
/// the outermost directory with Gradle settings inside the enclosing repository (or anywhere,
/// outside one), else the repository.
pub(crate) fn ensure_project_for_file(file: &Path, ctx: &mut AppContext) {
    if !ctx.has_singleton_model::<CodeIndexModel>() {
        return;
    }
    let holds_file = CodeIndexModel::as_ref(ctx)
        .project
        .as_ref()
        .is_some_and(|project| file.starts_with(&project.root));
    if holds_file {
        return;
    }
    let repository = file
        .ancestors()
        .skip(1)
        .find(|dir| dir.join(".git").exists());
    let gradle_root = file
        .ancestors()
        .skip(1)
        .take_while(|dir| repository.is_none_or(|repository| dir.starts_with(repository)))
        .filter(|dir| {
            dir.join("settings.gradle.kts").is_file() || dir.join("settings.gradle").is_file()
        })
        .last();
    match (gradle_root, repository) {
        (Some(root), _) => ensure_project(Some(root.to_path_buf()), true, ctx),
        // A repository is a project root even without build files at its top.
        (None, Some(repository)) => ensure_project(Some(repository.to_path_buf()), false, ctx),
        (None, None) => {}
    }
}

/// Resolves `request` against the index on the calling (background) thread.
pub(crate) fn navigate(handle: &IndexHandle, request: &CodeNavigationRequest) -> NavigationOutcome {
    let Some(rel_path) = request
        .path
        .strip_prefix(&handle.root)
        .ok()
        .and_then(|relative| relative.to_str())
        .map(|relative| relative.replace(std::path::MAIN_SEPARATOR, "/"))
    else {
        return NavigationOutcome::Nothing(
            "Open the file's project folder to navigate its code".to_owned(),
        );
    };
    let Some(language) = Language::of_editable_path(&request.path) else {
        return NavigationOutcome::Nothing(
            "Go to Definition works in Kotlin and Java files".to_owned(),
        );
    };
    let file = SourceFile {
        rel_path: &rel_path,
        text: &request.text,
        language,
    };
    let position = Position {
        line: request.line,
        column: request.column,
    };
    let indexing_note = if handle.state == IndexState::Indexing {
        " (still indexing, results may be incomplete)"
    } else {
        ""
    };
    let index = handle.index.read();
    if handle.state == IndexState::Indexing && index.stats().files == 0 {
        let project = handle
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return NavigationOutcome::Nothing(format!("Indexing {project}… try again in a moment"));
    }
    match request.kind {
        CodeNavigationKind::Definition => {
            let Some(result) = index.find_definition(&file, position) else {
                return NavigationOutcome::Nothing("No name at the cursor".to_owned());
            };
            let primary: Vec<&Definition> = result.primary().collect();
            let points_here = primary.iter().all(|definition| {
                definition.symbol.rel_path == rel_path
                    && definition.symbol.name_pos.line == request.line
            });
            if result.at_declaration && points_here {
                drop(index);
                // On a declaration, like the JetBrains IDEs, show where it's used instead.
                return navigate(
                    handle,
                    &CodeNavigationRequest {
                        kind: CodeNavigationKind::Usages,
                        ..request.clone()
                    },
                );
            }
            match primary.as_slice() {
                [] => NavigationOutcome::Nothing(format!(
                    "No declaration of {} in the project{indexing_note}",
                    result.name
                )),
                [only] => NavigationOutcome::Open {
                    path: index.abs_path(&only.symbol.rel_path),
                    position: only.symbol.name_pos,
                },
                several => NavigationOutcome::Show(NavigationResults {
                    root: handle.root.clone(),
                    summary: format!(
                        "{} declarations of {}{}{indexing_note}",
                        several.len(),
                        result.name,
                        if several[0].resolution.is_exact() {
                            ""
                        } else {
                            " matched by name"
                        }
                    ),
                    items: result
                        .definitions
                        .into_iter()
                        .map(NavigationItem::Definition)
                        .collect(),
                }),
            }
        }
        CodeNavigationKind::Usages => {
            let Some(result) = index.find_usages(&file, position, MAX_USAGES, USAGES_THREADS)
            else {
                return NavigationOutcome::Nothing("No name at the cursor".to_owned());
            };
            if result.targets.is_empty() {
                return NavigationOutcome::Nothing(format!(
                    "No declaration of {} in the project{indexing_note}",
                    result.name
                ));
            }
            let target = result.targets[0].qualified_name();
            if result.usages.is_empty() {
                return NavigationOutcome::Nothing(format!("No usages of {target}{indexing_note}"));
            }
            let count = result.usages.len();
            NavigationOutcome::Show(NavigationResults {
                root: handle.root.clone(),
                summary: format!(
                    "{count}{} usage{} of {target}{indexing_note}",
                    if result.complete { "" } else { "+" },
                    if count == 1 { "" } else { "s" },
                ),
                items: result
                    .usages
                    .into_iter()
                    .map(NavigationItem::Usage)
                    .collect(),
            })
        }
    }
}

fn is_indexable_root(root: &Path, require_build_files: bool) -> bool {
    const BUILD_FILES: &[&str] = &[
        "settings.gradle.kts",
        "settings.gradle",
        "build.gradle.kts",
        "build.gradle",
        "pom.xml",
    ];
    let too_broad = root.parent().is_none() || dirs::home_dir().is_some_and(|home| home == root);
    !too_broad && (!require_build_files || BUILD_FILES.iter().any(|file| root.join(file).is_file()))
}

impl CodeIndexModel {
    pub(crate) fn navigation_results(&self) -> Option<Arc<NavigationResults>> {
        self.navigation.clone()
    }

    pub(crate) fn set_navigation_results(&mut self, results: NavigationResults) {
        self.navigation = Some(Arc::new(results));
    }

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
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        project.watch = self.watch(&project.root, ctx);
        self.project = Some(project);
        if !self.start_build(ctx) {
            // Leave no project behind, so the next palette open tries again.
            self.close_project(ctx);
        }
        ctx.notify();
    }

    fn close_project(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(project) = self.project.take() else {
            return;
        };
        self.generation += 1;
        self.navigation = None;
        project.cancelled.store(true, Ordering::Relaxed);
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
    /// Returns whether the thread started.
    fn start_build(&self, ctx: &mut ModelContext<Self>) -> bool {
        let Some(project) = &self.project else {
            return false;
        };
        let (sender, receiver) = oneshot::channel();
        let root = project.root.clone();
        let snapshot = project.snapshot.clone();
        let index = project.index.clone();
        let cancelled = project.cancelled.clone();
        let spawned = std::thread::Builder::new()
            .name("code-index-build".to_owned())
            .spawn(move || {
                let report = build(&root, snapshot.as_deref(), &index, &cancelled);
                let _ = sender.send(report);
            });
        if let Err(err) = spawned {
            log::error!("[Code index] Couldn't start the indexing thread: {err}");
            return false;
        }
        let generation = self.generation;
        ctx.spawn(
            async move { receiver.await.ok() },
            move |model, report, ctx| {
                if model.generation != generation {
                    return;
                }
                if let Some(report) = report {
                    let snapshot = report.loaded_in.map_or_else(
                        || "no snapshot".to_owned(),
                        |loaded| format!("snapshot loaded in {} ms", loaded.as_millis()),
                    );
                    log::info!(
                        "[Code index] Indexed {} files, {} symbols in {} ms ({snapshot}, {} files \
                         parsed)",
                        report.files,
                        report.symbols,
                        report.total.as_millis(),
                        report.parsed
                    );
                }
                if let Some(project) = &mut model.project {
                    project.state = IndexState::Ready;
                }
                model.run_update(ctx);
                ctx.emit(CodeIndexEvent::Ready);
                ctx.notify();
            },
        );
        true
    }

    fn watch(
        &self,
        root: &Path,
        ctx: &mut ModelContext<Self>,
    ) -> Option<(WeakModelHandle<Repository>, SubscriberId)> {
        // Watch the enclosing repository, which the app usually watches already: the watcher
        // routes events to the deepest registered directory, so registering a subdirectory of a
        // watched repository would hide its events from the repository's other subscribers.
        let watched = root
            .ancestors()
            .find(|dir| dir.join(".git").exists())
            .unwrap_or(root);
        let standardized = StandardizedPath::from_local_canonicalized(watched).ok()?;
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
                // File I/O and fsync stay off the shared executor.
                let spawned = std::thread::Builder::new()
                    .name("code-index-save".to_owned())
                    .spawn(move || save_snapshot(&index, &snapshot));
                if let Err(err) = spawned {
                    log::warn!("[Code index] Couldn't start the snapshot thread: {err}");
                }
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
/// batches and saves the snapshot if anything changed. Stops early once `cancelled` is set.
fn build(
    root: &Path,
    snapshot: Option<&Path>,
    index: &RwLock<CodeIndex>,
    cancelled: &AtomicBool,
) -> BuildReport {
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
        if cancelled.load(Ordering::Relaxed) {
            return false;
        }
        let stats = index.write().apply(batch);
        changed += stats.files_parsed + stats.files_removed;
        true
    });
    if changed > 0
        && !cancelled.load(Ordering::Relaxed)
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

/// Encodes under the read lock and writes after releasing it, so searches and updates never
/// wait for the disk.
fn save_snapshot(index: &RwLock<CodeIndex>, snapshot: &Path) {
    let bytes = index.read().snapshot_bytes();
    if let Err(err) = write_snapshot(snapshot, &bytes) {
        log::warn!("[Code index] Couldn't save the index snapshot: {err}");
    }
}

fn is_generated_source(path: &Path) -> bool {
    let components: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
    components
        .windows(2)
        .any(|pair| pair[0] == "build" && pair[1] == "generated")
}

/// Kotlin and Java files, directories, and paths that no longer exist (deleted or moved files
/// and directories, whatever their names; removing an unindexed path is a no-op).
fn may_hold_sources(path: &Path) -> bool {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("kt" | "java") => true,
        _ => !path.is_file(),
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
