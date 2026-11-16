//! Warpium: the dialog that asks for the values of a toolbar command's `${prompt:Label}`
//! variables before it runs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pathfinder_geometry::vector::vec2f;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::Fill as ThemeFill;
use warp_editor::editor::NavigationKey;
use warpium_project_toolbar::{ToolbarButton, prompt_labels};
use warpui::elements::{
    Align, Border, ChildAnchor, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    Element, Flex, MainAxisAlignment, MainAxisSize, MouseStateHandle, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Entity, TypedActionView, View, ViewContext, ViewHandle};

use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys, SingleLineEditorOptions,
    TextColors, TextOptions,
};

const WIDTH: f32 = 440.;
const FONT_SIZE: f32 = 13.;

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        ToolbarPromptAction::Cancel,
        id!(ToolbarPromptDialog::ui_name()),
    )]);
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ToolbarPromptAction {
    Cancel,
    Run,
}

pub(crate) enum ToolbarPromptEvent {
    Close,
    /// Run `button` in the project at `root` with these answers, by label.
    Run {
        root: PathBuf,
        button: ToolbarButton,
        values: BTreeMap<String, String>,
    },
}

struct PendingRun {
    root: PathBuf,
    button: ToolbarButton,
    title: String,
}

#[derive(Default)]
struct MouseStates {
    cancel: MouseStateHandle,
    run: MouseStateHandle,
}

pub(crate) struct ToolbarPromptDialog {
    pending: Option<PendingRun>,
    fields: Vec<(String, ViewHandle<EditorView>)>,
    mouse_states: MouseStates,
}

impl ToolbarPromptDialog {
    pub(crate) fn new() -> Self {
        Self {
            pending: None,
            fields: Vec::new(),
            mouse_states: MouseStates::default(),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    /// Asks for the values of the prompt variables in `command`, then runs `button`.
    pub(crate) fn open(
        &mut self,
        root: PathBuf,
        button: ToolbarButton,
        command: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        self.fields = prompt_labels(command)
            .into_iter()
            .map(|label| {
                let editor = ctx.add_typed_action_view(|ctx| {
                    let appearance = Appearance::as_ref(ctx);
                    let theme = appearance.theme();
                    let options = SingleLineEditorOptions {
                        text: TextOptions {
                            font_family_override: Some(appearance.ui_font_family()),
                            text_colors_override: Some(TextColors {
                                default_color: theme.active_ui_text_color(),
                                disabled_color: theme.disabled_ui_text_color(),
                                hint_color: theme.disabled_ui_text_color(),
                            }),
                            ..Default::default()
                        },
                        propagate_and_no_op_vertical_navigation_keys:
                            PropagateAndNoOpNavigationKeys::Always,
                        ..Default::default()
                    };
                    EditorView::single_line(options, ctx)
                });
                ctx.subscribe_to_view(&editor, |me, editor, event, ctx| {
                    me.on_editor_event(&editor, event, ctx)
                });
                (label, editor)
            })
            .collect();
        self.pending = Some(PendingRun {
            title: button.display_label().to_owned(),
            root,
            button,
        });
        match self.fields.first() {
            Some((_, editor)) => ctx.focus(editor),
            None => ctx.focus_self(),
        }
        ctx.notify();
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        self.pending = None;
        self.fields.clear();
        ctx.emit(ToolbarPromptEvent::Close);
        ctx.notify();
    }

    fn run(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let values = self
            .fields
            .iter()
            .map(|(label, editor)| (label.clone(), editor.as_ref(ctx).buffer_text(ctx)))
            .collect();
        self.fields.clear();
        ctx.emit(ToolbarPromptEvent::Run {
            root: pending.root,
            button: pending.button,
            values,
        });
        ctx.notify();
    }

    fn on_editor_event(
        &mut self,
        editor: &ViewHandle<EditorView>,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(index) = self.fields.iter().position(|(_, field)| field == editor) else {
            return;
        };
        match event {
            EditorEvent::Enter if index + 1 == self.fields.len() => self.run(ctx),
            EditorEvent::Enter | EditorEvent::Navigate(NavigationKey::Tab) => {
                let next = (index + 1) % self.fields.len();
                ctx.focus(&self.fields[next].1);
            }
            EditorEvent::Navigate(NavigationKey::ShiftTab) => {
                let previous = (index + self.fields.len() - 1) % self.fields.len();
                ctx.focus(&self.fields[previous].1);
            }
            EditorEvent::Escape => self.close(ctx),
            _ => {}
        }
    }

    fn button(
        &self,
        label: &str,
        variant: ButtonVariant,
        mouse_state: &MouseStateHandle,
        action: ToolbarPromptAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .button(variant, mouse_state.clone())
            .with_text_label(label.to_owned())
            .with_style(UiComponentStyles {
                font_size: Some(FONT_SIZE),
                padding: Some(Coords::uniform(8.).left(14.).right(14.)),
                ..Default::default()
            })
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
            .finish()
    }
}

impl Entity for ToolbarPromptDialog {
    type Event = ToolbarPromptEvent;
}

impl TypedActionView for ToolbarPromptDialog {
    type Action = ToolbarPromptAction;

    fn handle_action(&mut self, action: &ToolbarPromptAction, ctx: &mut ViewContext<Self>) {
        match action {
            ToolbarPromptAction::Cancel => self.close(ctx),
            ToolbarPromptAction::Run => self.run(ctx),
        }
    }
}

impl View for ToolbarPromptDialog {
    fn ui_name() -> &'static str {
        "ToolbarPromptDialog"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let title = self
            .pending
            .as_ref()
            .map(|pending| format!("Run \"{}\"", pending.title))
            .unwrap_or_default();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(
                    Text::new(title, appearance.ui_font_family(), 15.)
                        .with_color(theme.active_ui_text_color().into_solid())
                        .with_style(Properties::default().weight(Weight::Bold))
                        .finish(),
                )
                .with_margin_bottom(12.)
                .finish(),
            );
        for (label, editor) in &self.fields {
            column.add_child(
                Container::new(
                    Text::new(label.clone(), appearance.ui_font_family(), 12.)
                        .with_color(theme.nonactive_ui_text_color().into_solid())
                        .finish(),
                )
                .with_margin_bottom(4.)
                .finish(),
            );
            column.add_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .text_input(editor.clone())
                        .build()
                        .finish(),
                )
                .with_margin_bottom(12.)
                .finish(),
            );
        }
        column.add_child(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_child(self.button(
                    "Cancel",
                    ButtonVariant::Secondary,
                    &self.mouse_states.cancel,
                    ToolbarPromptAction::Cancel,
                    appearance,
                ))
                .with_child(
                    Container::new(self.button(
                        "Run",
                        ButtonVariant::Accent,
                        &self.mouse_states.run,
                        ToolbarPromptAction::Run,
                        appearance,
                    ))
                    .with_margin_left(8.)
                    .finish(),
                )
                .finish(),
        );
        let card = ConstrainedBox::new(
            Container::new(column.finish())
                .with_uniform_padding(20.)
                .with_background(theme.surface_1())
                .with_border(Border::all(1.).with_border_fill(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                .finish(),
        )
        .with_width(WIDTH)
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
