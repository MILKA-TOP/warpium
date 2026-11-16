//! Warpium: the project toolbar's editor, a modal for adding, changing, reordering and hiding
//! toolbar buttons without editing `toolbar.toml` by hand. The model and the format-preserving
//! writes live in `warpium_project_toolbar::EditorModel`; this view is the form around it.

use std::path::{Path, PathBuf};

use pathfinder_geometry::vector::vec2f;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::Fill as ThemeFill;
use warp_core::ui::theme::color::internal_colors;
use warp_editor::editor::NavigationKey;
use warpium_project_toolbar::{
    BUNDLED_ICONS, BuiltinAction, ButtonSpec, COMMAND_VARIABLES, CommandTarget, ConfigSource,
    EditorModel, PROJECT_CONFIG_DIR, Placement, Scope, SpecAction, ToolbarButton,
    bundled_icon_path,
};
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{
    Align, Border, CacheOption, ChildAnchor, ChildView, ClippedScrollStateHandle,
    ClippedScrollable, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, Empty,
    Expanded, Fill, Flex, Hoverable, Icon as WarpUiIcon, Image, MainAxisAlignment, MainAxisSize,
    MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    Percentage, Radius, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Entity, TypedActionView, View, ViewContext, ViewHandle};

use super::project_toolbar::{ToolbarTexts, read_toolbar_texts, save_toolbar_plan};
use crate::editor::{
    EditorOptions, EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys,
    SingleLineEditorOptions, TextColors, TextOptions,
};
use crate::view_components::{Dropdown, DropdownItem, FilterableDropdown};

const MODAL_WIDTH: f32 = 880.;
const MODAL_HEIGHT_FRACTION: f32 = 0.86;
const LIST_WIDTH: f32 = 270.;
const PADDING: f32 = 20.;
const TITLE_FONT_SIZE: f32 = 16.;
const FONT_SIZE: f32 = 13.;
const LABEL_FONT_SIZE: f32 = 12.;
const ICON_SIZE: f32 = 16.;
const ROW_HEIGHT: f32 = 30.;
const ENV_REMOVE_WIDTH: f32 = 24.;

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        ProjectToolbarEditorAction::Cancel,
        id!(ProjectToolbarEditor::ui_name()),
    )]);
}

/// The kind of action the form edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionKind {
    Command,
    Workflow,
    LaunchConfig,
    TabConfig,
    Url,
    Builtin,
}

impl ActionKind {
    const ALL: [Self; 6] = [
        Self::Command,
        Self::Workflow,
        Self::LaunchConfig,
        Self::TabConfig,
        Self::Url,
        Self::Builtin,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Command => "Shell command",
            Self::Workflow => "Workflow",
            Self::LaunchConfig => "Launch configuration",
            Self::TabConfig => "Tab config",
            Self::Url => "URL",
            Self::Builtin => "Built-in action",
        }
    }

    fn of(action: &SpecAction) -> Self {
        match action {
            SpecAction::Command { .. } => Self::Command,
            SpecAction::Workflow(_) => Self::Workflow,
            SpecAction::LaunchConfig(_) => Self::LaunchConfig,
            SpecAction::TabConfig(_) => Self::TabConfig,
            SpecAction::Url(_) => Self::Url,
            SpecAction::Builtin(_) => Self::Builtin,
        }
    }

    /// What the single value field holds for this kind, as its label and placeholder.
    fn value_field(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Workflow => Some(("Workflow name", "Deploy to staging")),
            Self::LaunchConfig => Some(("Launch configuration name", "Android dev")),
            Self::TabConfig => Some(("Tab config name", "android-dev")),
            Self::Url => Some(("URL", "https://github.com/your-org/your-repo/actions")),
            Self::Command | Self::Builtin => None,
        }
    }
}

const TARGETS: [(CommandTarget, &str); 4] = [
    (CommandTarget::Current, "Current session"),
    (CommandTarget::NewTab, "New tab"),
    (CommandTarget::SplitRight, "Split right"),
    (CommandTarget::SplitDown, "Split down"),
];

const PLACEMENTS: [(Placement, &str); 3] = [
    (Placement::Toolbar, "Main toolbar"),
    (Placement::Overflow, "Overflow menu only"),
    (Placement::RunConfig, "Run configurations dropdown"),
];

const SCOPES: [(Scope, &str); 2] = [
    (Scope::Project, "Project (shared, .warpium/toolbar.toml)"),
    (Scope::Personal, "Personal (only me)"),
];

const NO_ICON: &str = "No icon";

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProjectToolbarEditorAction {
    Cancel,
    Save,
    TestRun,
    Select(usize),
    Add,
    Duplicate,
    Delete,
    Move(isize),
    ToggleHidden,
    SetKind(ActionKind),
    SetTarget(CommandTarget),
    SetPlacement(Placement),
    SetScope(Scope),
    SetIcon(Option<String>),
    SetBuiltin(String),
    AddEnv,
    RemoveEnv(usize),
    OpenFile { personal: bool },
}

pub(crate) enum ProjectToolbarEditorEvent {
    Close,
    /// The configs were written; the toolbar should reload.
    Saved,
    /// Run this (unsaved) button now.
    TestRun {
        root: PathBuf,
        button: ToolbarButton,
    },
    /// Open the team (or personal) config file as text.
    OpenFile {
        personal: bool,
    },
}

struct EnvRow {
    name: ViewHandle<EditorView>,
    value: ViewHandle<EditorView>,
    remove: MouseStateHandle,
}

#[derive(Default)]
struct MouseStates {
    close: MouseStateHandle,
    add: MouseStateHandle,
    duplicate: MouseStateHandle,
    delete: MouseStateHandle,
    move_up: MouseStateHandle,
    move_down: MouseStateHandle,
    hide: MouseStateHandle,
    add_env: MouseStateHandle,
    open_team: MouseStateHandle,
    open_personal: MouseStateHandle,
    test_run: MouseStateHandle,
    cancel: MouseStateHandle,
    save: MouseStateHandle,
}

/// The values of the selected button that live in dropdowns rather than editors.
#[derive(Debug, Clone, PartialEq)]
struct FormChoices {
    kind: ActionKind,
    target: CommandTarget,
    placement: Placement,
    scope: Scope,
    icon_name: Option<String>,
    builtin: Option<String>,
}

pub(crate) struct ProjectToolbarEditor {
    open: bool,
    root: Option<PathBuf>,
    /// The config files as they were when the editor opened; saving checks they are unchanged.
    texts: ToolbarTexts,
    model: EditorModel,
    selected: Option<usize>,
    /// Why the configs couldn't be loaded; the editor then only offers the raw files.
    load_error: Option<String>,
    error: Option<String>,
    /// Set while the form is filled from a button, so its own edit events don't write back.
    loading: bool,
    choices: FormChoices,
    label_editor: ViewHandle<EditorView>,
    tooltip_editor: ViewHandle<EditorView>,
    icon_path_editor: ViewHandle<EditorView>,
    command_editor: ViewHandle<EditorView>,
    cwd_editor: ViewHandle<EditorView>,
    value_editor: ViewHandle<EditorView>,
    env_rows: Vec<EnvRow>,
    kind_dropdown: ViewHandle<Dropdown<ProjectToolbarEditorAction>>,
    target_dropdown: ViewHandle<Dropdown<ProjectToolbarEditorAction>>,
    placement_dropdown: ViewHandle<Dropdown<ProjectToolbarEditorAction>>,
    scope_dropdown: ViewHandle<Dropdown<ProjectToolbarEditorAction>>,
    icon_dropdown: ViewHandle<FilterableDropdown<ProjectToolbarEditorAction>>,
    builtin_dropdown: ViewHandle<FilterableDropdown<ProjectToolbarEditorAction>>,
    row_mouse_states: Vec<MouseStateHandle>,
    list_scroll: ClippedScrollStateHandle,
    form_scroll: ClippedScrollStateHandle,
    mouse_states: MouseStates,
}

impl ProjectToolbarEditor {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let label_editor = Self::single_line_editor("Build", false, ctx);
        let tooltip_editor = Self::single_line_editor("Shown on hover", false, ctx);
        let icon_path_editor = Self::single_line_editor("icons/logo.svg", true, ctx);
        let cwd_editor = Self::single_line_editor(". (the project root)", true, ctx);
        let value_editor = Self::single_line_editor("", false, ctx);
        let command_editor = ctx.add_typed_action_view(|ctx| {
            let (font, colors) = Self::editor_style(true, ctx);
            let mut editor = EditorView::new(
                EditorOptions {
                    autogrow: true,
                    soft_wrap: true,
                    text: TextOptions {
                        font_family_override: Some(font),
                        text_colors_override: Some(colors),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ctx,
            );
            editor.set_placeholder_text("./gradlew assembleDebug", ctx);
            editor
        });
        for editor in [
            &label_editor,
            &tooltip_editor,
            &icon_path_editor,
            &cwd_editor,
            &value_editor,
            &command_editor,
        ] {
            ctx.subscribe_to_view(editor, |me, _, event, ctx| me.on_editor_event(event, ctx));
        }

        let kind_dropdown = Self::dropdown(
            ActionKind::ALL
                .iter()
                .map(|kind| {
                    DropdownItem::new(kind.label(), ProjectToolbarEditorAction::SetKind(*kind))
                })
                .collect(),
            ctx,
        );
        let target_dropdown = Self::dropdown(
            TARGETS
                .iter()
                .map(|(target, label)| {
                    DropdownItem::new(*label, ProjectToolbarEditorAction::SetTarget(*target))
                })
                .collect(),
            ctx,
        );
        let placement_dropdown = Self::dropdown(
            PLACEMENTS
                .iter()
                .map(|(placement, label)| {
                    DropdownItem::new(*label, ProjectToolbarEditorAction::SetPlacement(*placement))
                })
                .collect(),
            ctx,
        );
        let scope_dropdown = Self::dropdown(
            SCOPES
                .iter()
                .map(|(scope, label)| {
                    DropdownItem::new(*label, ProjectToolbarEditorAction::SetScope(*scope))
                })
                .collect(),
            ctx,
        );
        let icon_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = FilterableDropdown::new(ctx);
            let mut items = vec![DropdownItem::new(
                NO_ICON,
                ProjectToolbarEditorAction::SetIcon(None),
            )];
            items.extend(BUNDLED_ICONS.iter().map(|(name, _)| {
                DropdownItem::new(
                    *name,
                    ProjectToolbarEditorAction::SetIcon(Some((*name).to_owned())),
                )
            }));
            dropdown.set_items(items, ctx);
            dropdown
        });
        let builtin_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = FilterableDropdown::new(ctx);
            dropdown.set_items(
                BuiltinAction::ALL
                    .iter()
                    .map(|action| {
                        DropdownItem::new(
                            action.id(),
                            ProjectToolbarEditorAction::SetBuiltin(action.id().to_owned()),
                        )
                    })
                    .collect(),
                ctx,
            );
            dropdown
        });

        Self {
            open: false,
            root: None,
            texts: ToolbarTexts::default(),
            model: EditorModel::default(),
            selected: None,
            load_error: None,
            error: None,
            loading: false,
            choices: FormChoices {
                kind: ActionKind::Command,
                target: CommandTarget::Current,
                placement: Placement::Toolbar,
                scope: Scope::Project,
                icon_name: None,
                builtin: None,
            },
            label_editor,
            tooltip_editor,
            icon_path_editor,
            command_editor,
            cwd_editor,
            value_editor,
            env_rows: Vec::new(),
            kind_dropdown,
            target_dropdown,
            placement_dropdown,
            scope_dropdown,
            icon_dropdown,
            builtin_dropdown,
            row_mouse_states: Vec::new(),
            list_scroll: ClippedScrollStateHandle::default(),
            form_scroll: ClippedScrollStateHandle::default(),
            mouse_states: MouseStates::default(),
        }
    }

    fn editor_style(monospace: bool, ctx: &AppContext) -> (warpui::fonts::FamilyId, TextColors) {
        let appearance = Appearance::as_ref(ctx);
        let theme = appearance.theme();
        let font = if monospace {
            appearance.monospace_font_family()
        } else {
            appearance.ui_font_family()
        };
        let colors = TextColors {
            default_color: theme.active_ui_text_color(),
            disabled_color: theme.disabled_ui_text_color(),
            hint_color: theme.disabled_ui_text_color(),
        };
        (font, colors)
    }

    fn single_line_editor(
        placeholder: &'static str,
        monospace: bool,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<EditorView> {
        ctx.add_typed_action_view(move |ctx| {
            let (font, colors) = Self::editor_style(monospace, ctx);
            let options = SingleLineEditorOptions {
                text: TextOptions {
                    font_family_override: Some(font),
                    text_colors_override: Some(colors),
                    ..Default::default()
                },
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(placeholder, ctx);
            editor
        })
    }

    fn dropdown(
        items: Vec<DropdownItem<ProjectToolbarEditorAction>>,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<Dropdown<ProjectToolbarEditorAction>> {
        ctx.add_typed_action_view(move |ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_items(items, ctx);
            dropdown
        })
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the editor for the project at `root`, with the button `select` selected or, with
    /// `add`, a new button.
    pub(crate) fn open(
        &mut self,
        root: PathBuf,
        select: Option<String>,
        add: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.open = true;
        self.error = None;
        self.load_error = None;
        self.selected = None;
        self.list_scroll = ClippedScrollStateHandle::default();
        self.form_scroll = ClippedScrollStateHandle::default();
        let loaded = read_toolbar_texts(&root).and_then(|texts| {
            EditorModel::load(texts.team.as_deref(), texts.personal.as_deref())
                .map(|model| (texts, model))
                .map_err(|err| err.to_string())
        });
        self.root = Some(root);
        match loaded {
            Ok((texts, model)) => {
                self.texts = texts;
                self.model = model;
            }
            Err(message) => {
                self.texts = ToolbarTexts::default();
                self.model = EditorModel::default();
                self.load_error = Some(message);
            }
        }
        self.sync_row_mouse_states();
        if self.load_error.is_none() {
            if add {
                self.add_button(ctx);
            } else {
                let index = select
                    .and_then(|id| self.model.index_of(&id))
                    .or_else(|| (!self.model.buttons.is_empty()).then_some(0));
                self.select(index, ctx);
            }
        }
        ctx.focus_self();
        if self.selected.is_some() {
            ctx.focus(&self.label_editor);
        }
        ctx.notify();
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        self.open = false;
        self.env_rows.clear();
        ctx.emit(ProjectToolbarEditorEvent::Close);
        ctx.notify();
    }

    fn sync_row_mouse_states(&mut self) {
        self.row_mouse_states
            .resize_with(self.model.buttons.len(), MouseStateHandle::default);
    }

    fn selected_button(&self) -> Option<&warpium_project_toolbar::EditorButton> {
        self.selected
            .and_then(|index| self.model.buttons.get(index))
    }

    /// Selects a button and fills the form with it.
    fn select(&mut self, index: Option<usize>, ctx: &mut ViewContext<Self>) {
        self.commit_form(ctx);
        self.selected = index.filter(|index| *index < self.model.buttons.len());
        self.load_form(ctx);
        ctx.notify();
    }

    fn load_form(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(button) = self.selected_button().cloned() else {
            self.env_rows.clear();
            return;
        };
        self.loading = true;
        let spec = &button.spec;
        let is_custom_icon = spec
            .icon
            .as_deref()
            .is_some_and(|icon| bundled_icon_path(icon).is_none());
        let icon_name = spec.icon.clone().filter(|_| !is_custom_icon);
        let icon_path = spec.icon.clone().filter(|_| is_custom_icon);
        let (command, target, cwd, env, value, builtin) = match &spec.action {
            SpecAction::Command {
                command,
                target,
                cwd,
                env,
            } => (
                command.clone(),
                *target,
                cwd.clone(),
                env.clone(),
                String::new(),
                None,
            ),
            SpecAction::Workflow(value)
            | SpecAction::LaunchConfig(value)
            | SpecAction::TabConfig(value)
            | SpecAction::Url(value) => (
                String::new(),
                CommandTarget::Current,
                None,
                Vec::new(),
                value.clone(),
                None,
            ),
            SpecAction::Builtin(id) => (
                String::new(),
                CommandTarget::Current,
                None,
                Vec::new(),
                String::new(),
                Some(id.clone()),
            ),
        };
        let texts = [
            (&self.label_editor, spec.label.clone().unwrap_or_default()),
            (
                &self.tooltip_editor,
                spec.tooltip.clone().unwrap_or_default(),
            ),
            (&self.icon_path_editor, icon_path.unwrap_or_default()),
            (&self.command_editor, command),
            (&self.cwd_editor, cwd.unwrap_or_default()),
            (&self.value_editor, value),
        ];
        for (editor, text) in texts {
            editor.update(ctx, |editor, ctx| editor.set_buffer_text(&text, ctx));
        }
        self.choices = FormChoices {
            kind: ActionKind::of(&spec.action),
            target,
            placement: spec.placement,
            scope: button.scope,
            icon_name: icon_name.clone(),
            builtin: builtin.clone(),
        };
        let choices = self.choices.clone();
        self.kind_dropdown.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_action(ProjectToolbarEditorAction::SetKind(choices.kind), ctx)
        });
        self.target_dropdown.update(ctx, |dropdown, ctx| {
            dropdown
                .set_selected_by_action(ProjectToolbarEditorAction::SetTarget(choices.target), ctx)
        });
        self.placement_dropdown.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_action(
                ProjectToolbarEditorAction::SetPlacement(choices.placement),
                ctx,
            )
        });
        self.scope_dropdown.update(ctx, |dropdown, ctx| {
            dropdown
                .set_selected_by_action(ProjectToolbarEditorAction::SetScope(choices.scope), ctx)
        });
        self.icon_dropdown.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_name(icon_name.as_deref().unwrap_or(NO_ICON), ctx)
        });
        self.builtin_dropdown
            .update(ctx, |dropdown, ctx| match &builtin {
                Some(id) => dropdown.set_selected_by_name(id, ctx),
                None => dropdown.set_selected_by_name("", ctx),
            });
        self.env_rows = env
            .iter()
            .map(|(name, value)| self.new_env_row(name, value, ctx))
            .collect();
        self.loading = false;
    }

    fn new_env_row(&self, name: &str, value: &str, ctx: &mut ViewContext<Self>) -> EnvRow {
        let name_editor = Self::single_line_editor("NAME", true, ctx);
        let value_editor = Self::single_line_editor("value", true, ctx);
        for (editor, text) in [(&name_editor, name), (&value_editor, value)] {
            editor.update(ctx, |editor, ctx| editor.set_buffer_text(text, ctx));
            ctx.subscribe_to_view(editor, |me, _, event, ctx| me.on_editor_event(event, ctx));
        }
        EnvRow {
            name: name_editor,
            value: value_editor,
            remove: MouseStateHandle::default(),
        }
    }

    fn on_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_) | EditorEvent::BufferReplaced => {
                if !self.loading {
                    self.commit_form(ctx);
                    ctx.notify();
                }
            }
            EditorEvent::Escape => self.close(ctx),
            EditorEvent::Navigate(NavigationKey::Tab) | EditorEvent::Enter => {
                self.focus_adjacent_editor(1, ctx)
            }
            EditorEvent::Navigate(NavigationKey::ShiftTab) => self.focus_adjacent_editor(-1, ctx),
            _ => {}
        }
    }

    /// The text editors shown for the selected button, in form order.
    fn visible_editors(&self) -> Vec<ViewHandle<EditorView>> {
        let mut editors = vec![
            self.label_editor.clone(),
            self.tooltip_editor.clone(),
            self.icon_path_editor.clone(),
        ];
        match self.choices.kind {
            ActionKind::Command => {
                editors.push(self.command_editor.clone());
                editors.push(self.cwd_editor.clone());
                for row in &self.env_rows {
                    editors.push(row.name.clone());
                    editors.push(row.value.clone());
                }
            }
            ActionKind::Workflow
            | ActionKind::LaunchConfig
            | ActionKind::TabConfig
            | ActionKind::Url => editors.push(self.value_editor.clone()),
            ActionKind::Builtin => {}
        }
        editors
    }

    fn focus_adjacent_editor(&mut self, delta: isize, ctx: &mut ViewContext<Self>) {
        let editors = self.visible_editors();
        let Some(current) = editors.iter().position(|editor| editor.is_focused(ctx)) else {
            return;
        };
        let next = (current as isize + delta).rem_euclid(editors.len() as isize) as usize;
        ctx.focus(&editors[next]);
    }

    fn text(editor: &ViewHandle<EditorView>, ctx: &AppContext) -> String {
        editor.as_ref(ctx).buffer_text(ctx)
    }

    fn optional_text(editor: &ViewHandle<EditorView>, ctx: &AppContext) -> Option<String> {
        let text = Self::text(editor, ctx);
        (!text.trim().is_empty()).then(|| text.trim().to_owned())
    }

    /// Writes the form into the selected button.
    fn commit_form(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(index) = self.selected else {
            return;
        };
        let app: &AppContext = ctx;
        let choices = &self.choices;
        let action = match choices.kind {
            ActionKind::Command => SpecAction::Command {
                command: Self::text(&self.command_editor, app).trim_end().to_owned(),
                target: choices.target,
                cwd: Self::optional_text(&self.cwd_editor, app),
                env: self
                    .env_rows
                    .iter()
                    .map(|row| {
                        (
                            Self::text(&row.name, app).trim().to_owned(),
                            Self::text(&row.value, app),
                        )
                    })
                    .filter(|(name, value)| !name.is_empty() || !value.is_empty())
                    .collect(),
            },
            ActionKind::Workflow => SpecAction::Workflow(Self::text(&self.value_editor, app)),
            ActionKind::LaunchConfig => {
                SpecAction::LaunchConfig(Self::text(&self.value_editor, app))
            }
            ActionKind::TabConfig => SpecAction::TabConfig(Self::text(&self.value_editor, app)),
            ActionKind::Url => {
                SpecAction::Url(Self::text(&self.value_editor, app).trim().to_owned())
            }
            ActionKind::Builtin => SpecAction::Builtin(choices.builtin.clone().unwrap_or_default()),
        };
        let icon =
            Self::optional_text(&self.icon_path_editor, app).or_else(|| choices.icon_name.clone());
        let label = Self::optional_text(&self.label_editor, app);
        let tooltip = Self::optional_text(&self.tooltip_editor, app);
        let placement = choices.placement;
        let scope = choices.scope;
        let Some(button) = self.model.buttons.get_mut(index) else {
            return;
        };
        button.spec.label = label;
        button.spec.tooltip = tooltip;
        button.spec.icon = icon;
        button.spec.placement = placement;
        button.spec.action = action;
        let scope_changed = button.scope != scope;
        button.scope = scope;
        if scope_changed {
            let id = button.spec.id.clone();
            self.model.normalize();
            self.selected = self.model.index_of(&id);
        }
        self.error = None;
    }

    fn add_button(&mut self, ctx: &mut ViewContext<Self>) {
        self.commit_form(ctx);
        let id = self.model.unique_id("button");
        let mut spec = ButtonSpec::new_command(id);
        spec.label = Some("New button".to_owned());
        spec.icon = Some("terminal".to_owned());
        let index = self.model.add(spec, Scope::Project);
        self.sync_row_mouse_states();
        self.selected = None;
        self.select(Some(index), ctx);
        ctx.focus(&self.label_editor);
    }

    fn save(&mut self, ctx: &mut ViewContext<Self>) {
        self.commit_form(ctx);
        let Some(root) = self.root.clone() else {
            return;
        };
        let plan = match self
            .model
            .plan_save(self.texts.team.as_deref(), self.texts.personal.as_deref())
        {
            Ok(plan) => plan,
            Err(error) => {
                if let Some(index) = error.button.filter(|index| Some(*index) != self.selected) {
                    self.select(Some(index), ctx);
                }
                self.error = Some(error.message);
                ctx.notify();
                return;
            }
        };
        match save_toolbar_plan(&root, &self.texts, &plan, ctx) {
            Ok(()) => {
                ctx.emit(ProjectToolbarEditorEvent::Saved);
                self.close(ctx);
            }
            Err(message) => {
                self.error = Some(message);
                ctx.notify();
            }
        }
    }

    fn test_run(&mut self, ctx: &mut ViewContext<Self>) {
        self.commit_form(ctx);
        let (Some(root), Some(index), Some(button)) =
            (self.root.clone(), self.selected, self.selected_button())
        else {
            return;
        };
        // The team's own command still needs trust in the team file to run.
        let source = if self.model.runs_team_action(index) {
            ConfigSource::Team
        } else {
            ConfigSource::Personal
        };
        match button.spec.to_toolbar_button(source) {
            Ok(button) => {
                self.error = None;
                ctx.emit(ProjectToolbarEditorEvent::TestRun { root, button });
            }
            Err(message) => self.error = Some(message),
        }
        ctx.notify();
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let project = self
            .root
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new_inline(
                    "Customize toolbar",
                    appearance.ui_font_family(),
                    TITLE_FONT_SIZE,
                )
                .with_color(theme.active_ui_text_color().into_solid())
                .with_style(Properties::default().weight(Weight::Bold))
                .finish(),
            )
            .with_child(
                Shrinkable::new(
                    1.,
                    Container::new(
                        Text::new_inline(project, appearance.ui_font_family(), FONT_SIZE)
                            .with_color(theme.nonactive_ui_text_color().into_solid())
                            .finish(),
                    )
                    .with_margin_left(10.)
                    .finish(),
                )
                .finish(),
            )
            .with_child(
                appearance
                    .ui_builder()
                    .close_button(24., self.mouse_states.close.clone())
                    .build()
                    .on_click(|ctx, _, _| {
                        ctx.dispatch_typed_action(ProjectToolbarEditorAction::Cancel)
                    })
                    .finish(),
            )
            .finish()
    }

    fn render_list(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut rows = Flex::column();
        if self.model.buttons.is_empty() {
            rows.add_child(
                Container::new(
                    Text::new(
                        "No buttons yet. Add one to run any command from the title bar.",
                        appearance.ui_font_family(),
                        FONT_SIZE,
                    )
                    .with_color(theme.nonactive_ui_text_color().into_solid())
                    .soft_wrap(true)
                    .finish(),
                )
                .with_uniform_padding(8.)
                .finish(),
            );
        }
        let mut shown_personal_heading = false;
        for (index, button) in self.model.buttons.iter().enumerate() {
            if !self.model.is_team_located(index) && !shown_personal_heading {
                shown_personal_heading = true;
                if index > 0 {
                    rows.add_child(
                        Container::new(
                            ConstrainedBox::new(Empty::new().finish())
                                .with_height(1.)
                                .finish(),
                        )
                        .with_background(theme.outline())
                        .with_vertical_margin(4.)
                        .finish(),
                    );
                }
            }
            rows.add_child(self.render_list_row(index, button, appearance));
        }
        let list = ClippedScrollable::vertical(
            self.list_scroll.clone(),
            rows.finish(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish();

        let selected = self.selected;
        let can_move = |delta| selected.and_then(|index| self.model.move_target(index, delta));
        let hidden = self.selected_button().is_some_and(|button| button.hidden);
        let tools = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(self.small_button(
                "+ Add",
                true,
                &self.mouse_states.add,
                ProjectToolbarEditorAction::Add,
                appearance,
            ))
            .with_child(self.small_button(
                "Duplicate",
                selected.is_some(),
                &self.mouse_states.duplicate,
                ProjectToolbarEditorAction::Duplicate,
                appearance,
            ))
            .with_child(self.small_button(
                "Delete",
                selected.is_some(),
                &self.mouse_states.delete,
                ProjectToolbarEditorAction::Delete,
                appearance,
            ))
            .finish();
        let order = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(self.small_button(
                "Move up",
                can_move(-1).is_some(),
                &self.mouse_states.move_up,
                ProjectToolbarEditorAction::Move(-1),
                appearance,
            ))
            .with_child(self.small_button(
                "Move down",
                can_move(1).is_some(),
                &self.mouse_states.move_down,
                ProjectToolbarEditorAction::Move(1),
                appearance,
            ))
            .with_child(self.small_button(
                if hidden { "Show" } else { "Hide" },
                selected.is_some(),
                &self.mouse_states.hide,
                ProjectToolbarEditorAction::ToggleHidden,
                appearance,
            ))
            .finish();
        Flex::column()
            .with_child(section_label("Buttons", appearance))
            .with_child(
                Expanded::new(
                    1.,
                    Container::new(list)
                        .with_background(theme.surface_2())
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                        .with_uniform_padding(4.)
                        .finish(),
                )
                .finish(),
            )
            .with_child(Container::new(tools).with_margin_top(8.).finish())
            .with_child(Container::new(order).with_margin_top(4.).finish())
            .finish()
    }

    fn render_list_row(
        &self,
        index: usize,
        button: &warpium_project_toolbar::EditorButton,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let is_selected = self.selected == Some(index);
        let text_color = if button.hidden {
            theme.disabled_ui_text_color().into_solid()
        } else {
            theme.active_ui_text_color().into_solid()
        };
        let sub_color = theme.nonactive_ui_text_color().into_solid();
        let mut badges = Vec::new();
        badges.push(match button.scope {
            Scope::Project => "Project",
            Scope::Personal => "Personal",
        });
        match button.spec.placement {
            Placement::Toolbar => {}
            Placement::Overflow => badges.push("Overflow"),
            Placement::RunConfig => badges.push("Run config"),
        }
        if button.hidden {
            badges.push("Hidden");
        }
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(
                    ConstrainedBox::new(self.render_icon(&button.spec, text_color, appearance))
                        .with_width(ICON_SIZE)
                        .with_height(ICON_SIZE)
                        .finish(),
                )
                .with_margin_right(8.)
                .finish(),
            )
            .with_child(
                Shrinkable::new(
                    1.,
                    Text::new_inline(
                        button.spec.display_label().to_owned(),
                        appearance.ui_font_family(),
                        FONT_SIZE,
                    )
                    .with_color(text_color)
                    .finish(),
                )
                .finish(),
            )
            .with_child(
                Text::new_inline(badges.join(" · "), appearance.ui_font_family(), 11.)
                    .with_color(sub_color)
                    .finish(),
            )
            .finish();
        let mouse_state = self
            .row_mouse_states
            .get(index)
            .cloned()
            .unwrap_or_default();
        let background = is_selected.then(|| internal_colors::fg_overlay_3(theme));
        Hoverable::new(mouse_state, move |state| {
            let mut container =
                Container::new(ConstrainedBox::new(row).with_height(ROW_HEIGHT).finish())
                    .with_horizontal_padding(8.)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
            if let Some(background) = background {
                container = container.with_background(background);
            } else if state.is_hovered() {
                container = container.with_background(internal_colors::fg_overlay_2(theme));
            }
            container.finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ProjectToolbarEditorAction::Select(index))
        })
        .with_cursor(Cursor::PointingHand)
        .finish()
    }

    /// The button's icon, or its label's first letter when it has none.
    fn render_icon(
        &self,
        spec: &ButtonSpec,
        color: warpui::color::ColorU,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        match spec.icon.as_deref() {
            Some(name) if bundled_icon_path(name).is_some() => {
                WarpUiIcon::new(bundled_icon_path(name).unwrap_or_default(), color).finish()
            }
            Some(path) if self.root.is_some() => Image::new(
                AssetSource::LocalFile {
                    path: self
                        .root
                        .as_deref()
                        .unwrap_or(Path::new(""))
                        .join(PROJECT_CONFIG_DIR)
                        .join(path)
                        .to_string_lossy()
                        .into_owned(),
                    content_version: None,
                },
                CacheOption::BySize,
            )
            .contain()
            .finish(),
            Some(_) | None => Align::new(
                Text::new_inline(
                    spec.display_label()
                        .chars()
                        .next()
                        .map(|first| first.to_uppercase().to_string())
                        .unwrap_or_default(),
                    appearance.ui_font_family(),
                    11.,
                )
                .with_color(color)
                .finish(),
            )
            .finish(),
        }
    }

    fn render_form(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let Some(button) = self.selected_button() else {
            return Align::new(
                Text::new_inline(
                    "Select a button, or add one.",
                    appearance.ui_font_family(),
                    FONT_SIZE,
                )
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .finish(),
            )
            .finish();
        };
        let mut form = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        form.add_child(field(
            "Label",
            self.text_input(&self.label_editor, appearance),
            appearance,
        ));
        form.add_child(field(
            "Tooltip",
            self.text_input(&self.tooltip_editor, appearance),
            appearance,
        ));
        let icon_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(
                    ConstrainedBox::new(self.render_icon(
                        &button.spec,
                        theme.active_ui_text_color().into_solid(),
                        appearance,
                    ))
                    .with_width(ICON_SIZE)
                    .with_height(ICON_SIZE)
                    .finish(),
                )
                .with_margin_right(8.)
                .finish(),
            )
            .with_child(Expanded::new(1., ChildView::new(&self.icon_dropdown).finish()).finish())
            .with_child(
                Expanded::new(
                    1.,
                    Container::new(self.text_input(&self.icon_path_editor, appearance))
                        .with_margin_left(8.)
                        .finish(),
                )
                .finish(),
            )
            .finish();
        form.add_child(field(
            "Icon (search the icon set, or enter an .svg path relative to .warpium/)",
            icon_row,
            appearance,
        ));
        form.add_child(field(
            "Action",
            ChildView::new(&self.kind_dropdown).finish(),
            appearance,
        ));
        match self.choices.kind {
            ActionKind::Command => {
                form.add_child(field(
                    "Command",
                    ConstrainedBox::new(self.text_input(&self.command_editor, appearance))
                        .with_min_height(56.)
                        .finish(),
                    appearance,
                ));
                form.add_child(hint(&variables_hint(), appearance));
                form.add_child(field(
                    "Run in",
                    ChildView::new(&self.target_dropdown).finish(),
                    appearance,
                ));
                form.add_child(field(
                    "Working directory (relative to the project root)",
                    self.text_input(&self.cwd_editor, appearance),
                    appearance,
                ));
                form.add_child(field(
                    "Environment variables",
                    self.render_env(appearance),
                    appearance,
                ));
            }
            ActionKind::Builtin => form.add_child(field(
                "Built-in action",
                ChildView::new(&self.builtin_dropdown).finish(),
                appearance,
            )),
            kind @ (ActionKind::Workflow
            | ActionKind::LaunchConfig
            | ActionKind::TabConfig
            | ActionKind::Url) => {
                let (label, _) = kind.value_field().unwrap_or(("Value", ""));
                form.add_child(field(
                    label,
                    self.text_input(&self.value_editor, appearance),
                    appearance,
                ));
            }
        }
        form.add_child(field(
            "Show in",
            ChildView::new(&self.placement_dropdown).finish(),
            appearance,
        ));
        form.add_child(field(
            "Save to",
            ChildView::new(&self.scope_dropdown).finish(),
            appearance,
        ));
        let scope_hint = match (button.scope, self.model.is_in_team_file(&button.spec.id)) {
            (Scope::Personal, true) => {
                "Saved as your personal override of the project's button; the shared file is \
                 left as it is."
            }
            (Scope::Personal, false) => "Only you see this button.",
            (Scope::Project, _) => {
                "Shared with everyone who uses the project. Your own edits here don't ask for \
                 trust again."
            }
        };
        form.add_child(hint(scope_hint, appearance));
        form.add_child(hint(&format!("Id: {}", button.spec.id), appearance));
        ClippedScrollable::vertical(
            self.form_scroll.clone(),
            Container::new(form.finish())
                .with_margin_right(12.)
                .finish(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish()
    }

    fn render_env(&self, appearance: &Appearance) -> Box<dyn Element> {
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (index, row) in self.env_rows.iter().enumerate() {
            column.add_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Expanded::new(1., self.text_input(&row.name, appearance)).finish(),
                        )
                        .with_child(
                            Expanded::new(
                                2.,
                                Container::new(self.text_input(&row.value, appearance))
                                    .with_margin_left(8.)
                                    .finish(),
                            )
                            .finish(),
                        )
                        .with_child(
                            ConstrainedBox::new(
                                Container::new(
                                    appearance
                                        .ui_builder()
                                        .close_button(20., row.remove.clone())
                                        .build()
                                        .on_click(move |ctx, _, _| {
                                            ctx.dispatch_typed_action(
                                                ProjectToolbarEditorAction::RemoveEnv(index),
                                            )
                                        })
                                        .finish(),
                                )
                                .with_margin_left(4.)
                                .finish(),
                            )
                            .with_width(ENV_REMOVE_WIDTH)
                            .finish(),
                        )
                        .finish(),
                )
                .with_margin_bottom(6.)
                .finish(),
            );
        }
        column.add_child(
            Align::new(self.small_button(
                "+ Add variable",
                true,
                &self.mouse_states.add_env,
                ProjectToolbarEditorAction::AddEnv,
                appearance,
            ))
            .left()
            .finish(),
        );
        column.finish()
    }

    /// The toolbar as it will look: inline buttons, then a "…" for the overflow menu's buttons
    /// and the run configurations.
    fn render_preview(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = theme.sub_text_color(theme.background()).into_solid();
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        let shown = self.model.buttons.iter().filter(|button| !button.hidden);
        let mut overflow = 0;
        let mut run_configs = Vec::new();
        for button in shown {
            match button.spec.placement {
                Placement::Toolbar => {
                    let content = if button.spec.icon.is_some() {
                        ConstrainedBox::new(self.render_icon(&button.spec, color, appearance))
                            .with_width(ICON_SIZE)
                            .with_height(ICON_SIZE)
                            .finish()
                    } else {
                        Text::new_inline(
                            button.spec.display_label().to_owned(),
                            appearance.ui_font_family(),
                            12.,
                        )
                        .with_color(color)
                        .finish()
                    };
                    row.add_child(
                        Container::new(content)
                            .with_horizontal_padding(5.)
                            .with_vertical_padding(4.)
                            .with_margin_right(2.)
                            .finish(),
                    );
                }
                Placement::Overflow => overflow += 1,
                Placement::RunConfig => run_configs.push(button.spec.display_label().to_owned()),
            }
        }
        if overflow > 0 {
            row.add_child(
                Container::new(
                    ConstrainedBox::new(Icon::DotsHorizontal.to_warpui_icon(color.into()).finish())
                        .with_width(ICON_SIZE)
                        .with_height(ICON_SIZE)
                        .finish(),
                )
                .with_horizontal_padding(5.)
                .finish(),
            );
        }
        if let Some(first) = run_configs.first() {
            let label = if run_configs.len() > 1 {
                format!("{first} ▾")
            } else {
                first.clone()
            };
            row.add_child(
                Container::new(
                    Text::new_inline(label, appearance.ui_font_family(), 12.)
                        .with_color(theme.active_ui_text_color().into_solid())
                        .finish(),
                )
                .with_margin_left(8.)
                .finish(),
            );
            row.add_child(
                Container::new(
                    ConstrainedBox::new(
                        WarpUiIcon::new(Icon::Play.into(), theme.ansi_fg_green()).finish(),
                    )
                    .with_width(ICON_SIZE)
                    .with_height(ICON_SIZE)
                    .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            );
        }
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(section_label("Preview", appearance))
                    .with_margin_right(12.)
                    .finish(),
            )
            .with_child(
                Shrinkable::new(
                    1.,
                    Container::new(row.finish())
                        .with_background(theme.background())
                        .with_border(Border::all(1.).with_border_fill(theme.outline()))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                        .with_horizontal_padding(8.)
                        .with_vertical_padding(4.)
                        .finish(),
                )
                .finish(),
            )
            .finish()
    }

    fn render_footer(&self, appearance: &Appearance) -> Box<dyn Element> {
        let is_command = self.selected.is_some() && self.choices.kind == ActionKind::Command;
        let can_save = self.load_error.is_none();
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Flex::row()
                    .with_child(self.small_button(
                        "Open project TOML",
                        self.root.is_some(),
                        &self.mouse_states.open_team,
                        ProjectToolbarEditorAction::OpenFile { personal: false },
                        appearance,
                    ))
                    .with_child(self.small_button(
                        "Open personal TOML",
                        self.root.is_some(),
                        &self.mouse_states.open_personal,
                        ProjectToolbarEditorAction::OpenFile { personal: true },
                        appearance,
                    ))
                    .finish(),
            )
            .with_child(
                Flex::row()
                    .with_child(self.footer_button(
                        "Test run",
                        ButtonVariant::Secondary,
                        is_command,
                        &self.mouse_states.test_run,
                        ProjectToolbarEditorAction::TestRun,
                        appearance,
                    ))
                    .with_child(self.footer_button(
                        "Cancel",
                        ButtonVariant::Secondary,
                        true,
                        &self.mouse_states.cancel,
                        ProjectToolbarEditorAction::Cancel,
                        appearance,
                    ))
                    .with_child(self.footer_button(
                        "Save",
                        ButtonVariant::Accent,
                        can_save,
                        &self.mouse_states.save,
                        ProjectToolbarEditorAction::Save,
                        appearance,
                    ))
                    .finish(),
            )
            .finish()
    }

    fn text_input(
        &self,
        editor: &ViewHandle<EditorView>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .text_input(editor.clone())
            .build()
            .finish()
    }

    fn small_button(
        &self,
        label: &str,
        enabled: bool,
        mouse_state: &MouseStateHandle,
        action: ProjectToolbarEditorAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mut button = appearance
            .ui_builder()
            .button(ButtonVariant::Text, mouse_state.clone())
            .with_text_label(label.to_owned())
            .with_style(UiComponentStyles {
                font_size: Some(LABEL_FONT_SIZE),
                padding: Some(Coords::uniform(4.).left(6.).right(6.)),
                ..Default::default()
            });
        if !enabled {
            button = button.disabled();
        }
        Container::new(
            button
                .build()
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                .finish(),
        )
        .with_margin_right(2.)
        .finish()
    }

    fn footer_button(
        &self,
        label: &str,
        variant: ButtonVariant,
        enabled: bool,
        mouse_state: &MouseStateHandle,
        action: ProjectToolbarEditorAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mut button = appearance
            .ui_builder()
            .button(variant, mouse_state.clone())
            .with_text_label(label.to_owned())
            .with_style(UiComponentStyles {
                font_size: Some(FONT_SIZE),
                padding: Some(Coords::uniform(8.).left(14.).right(14.)),
                ..Default::default()
            });
        if !enabled {
            button = button.disabled();
        }
        Container::new(
            button
                .build()
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                .finish(),
        )
        .with_margin_left(8.)
        .finish()
    }
}

fn variables_hint() -> String {
    let names = COMMAND_VARIABLES
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join("  ");
    format!(
        "Variables (quoted for your shell, so don't wrap them in quotes): {names}. $${{ is a \
         literal ${{."
    )
}

fn section_label(text: &str, appearance: &Appearance) -> Box<dyn Element> {
    Container::new(
        Text::new_inline(
            text.to_owned(),
            appearance.ui_font_family(),
            LABEL_FONT_SIZE,
        )
        .with_color(appearance.theme().active_ui_text_color().into_solid())
        .with_style(Properties::default().weight(Weight::Semibold))
        .finish(),
    )
    .with_margin_bottom(6.)
    .finish()
}

fn field(label: &str, control: Box<dyn Element>, appearance: &Appearance) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(
                    Text::new(
                        label.to_owned(),
                        appearance.ui_font_family(),
                        LABEL_FONT_SIZE,
                    )
                    .with_color(appearance.theme().nonactive_ui_text_color().into_solid())
                    .soft_wrap(true)
                    .finish(),
                )
                .with_margin_bottom(4.)
                .finish(),
            )
            .with_child(control)
            .finish(),
    )
    .with_margin_bottom(12.)
    .finish()
}

fn hint(text: &str, appearance: &Appearance) -> Box<dyn Element> {
    Container::new(
        Text::new(text.to_owned(), appearance.ui_font_family(), 11.)
            .with_color(appearance.theme().nonactive_ui_text_color().into_solid())
            .soft_wrap(true)
            .finish(),
    )
    .with_margin_top(-6.)
    .with_margin_bottom(12.)
    .finish()
}

impl Entity for ProjectToolbarEditor {
    type Event = ProjectToolbarEditorEvent;
}

impl TypedActionView for ProjectToolbarEditor {
    type Action = ProjectToolbarEditorAction;

    fn handle_action(&mut self, action: &ProjectToolbarEditorAction, ctx: &mut ViewContext<Self>) {
        match action {
            ProjectToolbarEditorAction::Cancel => self.close(ctx),
            ProjectToolbarEditorAction::Save => self.save(ctx),
            ProjectToolbarEditorAction::TestRun => self.test_run(ctx),
            ProjectToolbarEditorAction::Select(index) => self.select(Some(*index), ctx),
            ProjectToolbarEditorAction::Add => self.add_button(ctx),
            ProjectToolbarEditorAction::Duplicate => {
                self.commit_form(ctx);
                if let Some(index) = self.selected.and_then(|index| self.model.duplicate(index)) {
                    self.sync_row_mouse_states();
                    self.selected = None;
                    self.select(Some(index), ctx);
                }
            }
            ProjectToolbarEditorAction::Delete => {
                if let Some(index) = self.selected {
                    self.model.remove(index);
                    self.sync_row_mouse_states();
                    self.selected = None;
                    let next = index.min(self.model.buttons.len().saturating_sub(1));
                    self.select((!self.model.buttons.is_empty()).then_some(next), ctx);
                }
            }
            ProjectToolbarEditorAction::Move(delta) => {
                self.commit_form(ctx);
                if let Some(index) = self.selected
                    && let Some(target) = self.model.move_button(index, *delta)
                {
                    self.selected = Some(target);
                    ctx.notify();
                }
            }
            ProjectToolbarEditorAction::ToggleHidden => {
                if let Some(button) = self
                    .selected
                    .and_then(|index| self.model.buttons.get_mut(index))
                {
                    button.hidden = !button.hidden;
                    ctx.notify();
                }
            }
            ProjectToolbarEditorAction::SetKind(kind) => {
                self.choices.kind = *kind;
                if *kind == ActionKind::Builtin && self.choices.builtin.is_none() {
                    let first = BuiltinAction::ALL[0].id().to_owned();
                    self.builtin_dropdown.update(ctx, |dropdown, ctx| {
                        dropdown.set_selected_by_name(&first, ctx)
                    });
                    self.choices.builtin = Some(first);
                }
                if let Some((_, placeholder)) = kind.value_field() {
                    self.value_editor.update(ctx, |editor, ctx| {
                        editor.set_placeholder_text(placeholder, ctx)
                    });
                }
                self.commit_form(ctx);
                ctx.notify();
            }
            ProjectToolbarEditorAction::SetTarget(target) => {
                self.choices.target = *target;
                self.commit_form(ctx);
                ctx.notify();
            }
            ProjectToolbarEditorAction::SetPlacement(placement) => {
                self.choices.placement = *placement;
                self.commit_form(ctx);
                ctx.notify();
            }
            ProjectToolbarEditorAction::SetScope(scope) => {
                self.choices.scope = *scope;
                self.commit_form(ctx);
                ctx.notify();
            }
            ProjectToolbarEditorAction::SetIcon(name) => {
                self.choices.icon_name = name.clone();
                if name.is_some() {
                    self.icon_path_editor
                        .update(ctx, |editor, ctx| editor.set_buffer_text("", ctx));
                }
                self.commit_form(ctx);
                ctx.notify();
            }
            ProjectToolbarEditorAction::SetBuiltin(id) => {
                self.choices.builtin = Some(id.clone());
                self.commit_form(ctx);
                ctx.notify();
            }
            ProjectToolbarEditorAction::AddEnv => {
                let row = self.new_env_row("", "", ctx);
                ctx.focus(&row.name);
                self.env_rows.push(row);
                ctx.notify();
            }
            ProjectToolbarEditorAction::RemoveEnv(index) => {
                if *index < self.env_rows.len() {
                    self.env_rows.remove(*index);
                    self.commit_form(ctx);
                    ctx.notify();
                }
            }
            ProjectToolbarEditorAction::OpenFile { personal } => {
                self.close(ctx);
                ctx.emit(ProjectToolbarEditorEvent::OpenFile {
                    personal: *personal,
                });
            }
        }
    }
}

impl View for ProjectToolbarEditor {
    fn ui_name() -> &'static str {
        "ProjectToolbarEditor"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(self.render_header(appearance))
                    .with_margin_bottom(14.)
                    .finish(),
            );
        match &self.load_error {
            Some(message) => column.add_child(
                Expanded::new(
                    1.,
                    Text::new(
                        format!(
                            "The toolbar config can't be edited here until it is fixed:\n{message}"
                        ),
                        appearance.ui_font_family(),
                        FONT_SIZE,
                    )
                    .with_color(theme.ui_error_color())
                    .soft_wrap(true)
                    .finish(),
                )
                .finish(),
            ),
            None => column.add_child(
                Expanded::new(
                    1.,
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_child(
                            ConstrainedBox::new(self.render_list(appearance))
                                .with_width(LIST_WIDTH)
                                .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.,
                                Container::new(self.render_form(appearance))
                                    .with_margin_left(PADDING)
                                    .finish(),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .finish(),
            ),
        }
        column.add_child(
            Container::new(self.render_preview(appearance))
                .with_margin_top(14.)
                .finish(),
        );
        if let Some(error) = &self.error {
            column.add_child(
                Container::new(
                    Text::new(error.clone(), appearance.ui_font_family(), FONT_SIZE)
                        .with_color(theme.ui_error_color())
                        .soft_wrap(true)
                        .finish(),
                )
                .with_margin_top(10.)
                .finish(),
            );
        }
        column.add_child(
            Container::new(self.render_footer(appearance))
                .with_margin_top(14.)
                .finish(),
        );

        let card = Percentage::height(
            MODAL_HEIGHT_FRACTION,
            ConstrainedBox::new(
                Container::new(column.finish())
                    .with_uniform_padding(PADDING)
                    .with_background(theme.surface_1())
                    .with_border(Border::all(1.).with_border_fill(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                    .finish(),
            )
            .with_width(MODAL_WIDTH)
            .finish(),
        )
        .finish();
        let mut stack = Stack::new();
        stack.add_positioned_child(
            card,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 0.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            ),
        );
        Container::new(Align::new(stack.finish()).finish())
            .with_background_color(ThemeFill::blur().into())
            .finish()
    }
}
