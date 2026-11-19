//! Warpium: Git Blame annotations, a column left of the code editor's gutter with the commit
//! date and author of each line. It wraps the editor's scrollable and reads line positions from
//! the render model, so annotations stay aligned with soft-wrapped lines and inline diffs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warp_core::ui::appearance::Appearance;
use warp_editor::render::model::{LineCount, RenderState};
use warpium_git::blame::Blame;
use warpium_git::format::{local_date, local_date_time, relative_time, truncate_chars};
use warpui::elements::{
    ChildAnchor, ConstrainedBox, Container, Hoverable, MouseStateHandle, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Point, Stack, Text,
};
use warpui::event::DispatchedEvent;
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::{
    AfterLayoutContext, AppContext, ClipBounds, Element, Event, EventContext, LayoutContext,
    ModelHandle, PaintContext, SingletonEntity, SizeConstraint,
};

use crate::code::local_code_editor::LocalCodeEditorAction;
use crate::warpium::git_tools::EditorGitAction;

const FONT_SIZE: f32 = 11.;
const AUTHOR_CHARS: usize = 14;
/// Lines annotated per frame, at most (a very tall window at a tiny font size).
const MAX_ROWS: usize = 1_000;

/// What the column shows; cheap to clone into every frame's element.
#[derive(Clone)]
pub struct LineAnnotations {
    pub blame: Arc<Blame>,
    /// Background opacity of each commit (by index in the blame), stronger for newer commits.
    pub age_alpha: Arc<Vec<u8>>,
    /// Hover state of each annotated line, kept across frames.
    pub mouse_states: Rc<RefCell<HashMap<usize, MouseStateHandle>>>,
    pub width: f32,
    /// Unix seconds, for "3 days ago" in tooltips.
    pub now: i64,
}

impl LineAnnotations {
    pub fn new(blame: Arc<Blame>, now: i64) -> Self {
        let age_alpha = Arc::new(age_alphas(&blame));
        // Room for "2026-11-19 " plus the author, in the UI font's average advance.
        let author_chars = blame.max_author_chars().clamp(6, AUTHOR_CHARS);
        let width = (11 + author_chars) as f32 * FONT_SIZE * 0.6 + 12.;
        Self {
            blame,
            age_alpha,
            mouse_states: Rc::new(RefCell::new(HashMap::new())),
            width,
            now,
        }
    }

    fn build_row(&self, line: usize, height: f32, app: &AppContext) -> Option<Box<dyn Element>> {
        let commit_index = self.blame.commit_index(line)?;
        let commit = self.blame.line(line)?;
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let first_of_run = line == 0 || self.blame.commit_index(line - 1) != Some(commit_index);
        let (text, color) = if commit.is_uncommitted() {
            (
                "Not committed".to_owned(),
                theme.hint_text_color(theme.background()).into_solid(),
            )
        } else {
            (
                format!(
                    "{} {}",
                    local_date(commit.author_time),
                    truncate_chars(&commit.author, AUTHOR_CHARS)
                ),
                theme.sub_text_color(theme.background()).into_solid(),
            )
        };
        let text = if first_of_run { text } else { String::new() };
        let mut background = theme.accent().into_solid();
        background.a = self.age_alpha.get(commit_index).copied().unwrap_or(0);
        let tooltip = if commit.is_uncommitted() {
            appearance
                .ui_builder()
                .tool_tip("Not committed yet".to_owned())
                .build()
                .finish()
        } else {
            appearance
                .ui_builder()
                .tool_tip_with_sublabel(
                    format!(
                        "{} {} · {} ({})",
                        commit.oid.short(),
                        commit.author,
                        local_date_time(commit.author_time),
                        relative_time(commit.author_time, self.now)
                    ),
                    commit.summary.clone(),
                )
                .build()
                .finish()
        };
        let width = self.width;
        let font = appearance.ui_font_family();
        let mouse_state = self
            .mouse_states
            .borrow_mut()
            .entry(line)
            .or_default()
            .clone();
        let row = Hoverable::new(mouse_state, move |state| {
            let label = Container::new(
                ConstrainedBox::new(
                    Text::new_inline(text, font, FONT_SIZE)
                        .with_color(color)
                        .with_clip(ClipConfig::end())
                        .finish(),
                )
                .with_width(width - 8.)
                .with_height(height)
                .finish(),
            )
            .with_padding_left(6.)
            .with_background_color(background)
            .finish();
            let mut stack = Stack::new().with_child(label);
            if state.is_hovered() {
                stack.add_positioned_overlay_child(
                    tooltip,
                    OffsetPositioning::offset_from_parent(
                        vec2f(width + 4., 0.),
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::TopLeft,
                        ChildAnchor::TopLeft,
                    ),
                );
            }
            stack.finish()
        });
        if commit.is_uncommitted() {
            return Some(row.finish());
        }
        let oid = commit.oid.to_hex();
        Some(
            row.on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(LocalCodeEditorAction::WarpiumGit(
                    EditorGitAction::ShowCommit(oid.clone()),
                ))
            })
            .with_cursor(Cursor::PointingHand)
            .finish(),
        )
    }
}

/// Background opacity per commit: the newest commit gets the most, the oldest none.
fn age_alphas(blame: &Blame) -> Vec<u8> {
    const MAX_ALPHA: f32 = 56.;
    let commits = blame.commits();
    let mut order: Vec<usize> = (0..commits.len()).collect();
    order.sort_by_key(|index| commits[*index].author_time);
    let mut alphas = vec![0; commits.len()];
    let last = order.len().saturating_sub(1).max(1) as f32;
    for (rank, index) in order.into_iter().enumerate() {
        alphas[index] = if commits[index].is_uncommitted() {
            0
        } else {
            (rank as f32 / last * MAX_ALPHA) as u8
        };
    }
    alphas
}

/// The editor's scrollable with the annotation column on its left.
pub struct BlameColumn {
    child: Box<dyn Element>,
    render_state: ModelHandle<RenderState>,
    annotations: LineAnnotations,
    /// Annotation rows of the visible lines, with their offsets from the top.
    rows: Vec<(f32, Box<dyn Element>)>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl BlameColumn {
    pub fn new(
        child: Box<dyn Element>,
        render_state: ModelHandle<RenderState>,
        annotations: LineAnnotations,
    ) -> Self {
        Self {
            child,
            render_state,
            annotations,
            rows: Vec::new(),
            size: None,
            origin: None,
        }
    }
}

impl Element for BlameColumn {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let width = self.annotations.width;
        let child_constraint = SizeConstraint::new(
            (constraint.min - vec2f(width, 0.)).max(Vector2F::zero()),
            (constraint.max - vec2f(width, 0.)).max(Vector2F::zero()),
        );
        let child_size = self.child.layout(child_constraint, ctx, app);
        let size = vec2f(child_size.x() + width, child_size.y());
        self.size = Some(size);

        self.rows.clear();
        let state = self.render_state.as_ref(app);
        let scroll_top = state.viewport().scroll_top().as_f32();
        let content = state.content();
        let line_top = |line: usize| content.y_offset_at_line(LineCount::from(line)).as_f32();
        let line_count = self.annotations.blame.line_count();
        let (mut low, mut high) = (0, line_count);
        while low < high {
            let middle = (low + high) / 2;
            if line_top(middle + 1) <= scroll_top {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let bottom = scroll_top + size.y();
        let mut line = low;
        while line < line_count && self.rows.len() < MAX_ROWS {
            let top = line_top(line);
            if top >= bottom {
                break;
            }
            let height = line_top(line + 1) - top;
            if height > 0.
                && let Some(mut row) = self.annotations.build_row(line, height, app)
            {
                row.layout(SizeConstraint::strict(vec2f(width, height)), ctx, app);
                self.rows.push((top - scroll_top, row));
            }
            line += 1;
        }
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
        for (_, row) in &mut self.rows {
            row.after_layout(ctx, app);
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let width = self.annotations.width;
        self.child.paint(origin + vec2f(width, 0.), ctx, app);
        let height = self.size.map(|size| size.y()).unwrap_or_default();
        ctx.scene
            .start_layer(ClipBounds::BoundedByActiveLayerAnd(RectF::new(
                origin,
                vec2f(width, height),
            )));
        for (offset, row) in &mut self.rows {
            row.paint(origin + vec2f(0., *offset), ctx, app);
        }
        ctx.scene.stop_layer();
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let mut handled = false;
        for (_, row) in &mut self.rows {
            handled |= row.dispatch_event(event, ctx, app);
        }
        // The wheel over the annotations scrolls the editor next to them.
        if let (Event::ScrollWheel { position, .. }, Some(origin), Some(size)) =
            (event.raw_event(), self.origin, self.size)
        {
            let width = self.annotations.width;
            let column = RectF::new(origin.xy(), vec2f(width, size.y()));
            if column.contains_point(*position) {
                let mut forwarded = event.raw_event().clone();
                if let Event::ScrollWheel { position, .. } = &mut forwarded {
                    *position = vec2f(origin.xy().x() + width + 1., position.y());
                }
                return self
                    .child
                    .dispatch_event(&DispatchedEvent::from(forwarded), ctx, app)
                    || handled;
            }
        }
        self.child.dispatch_event(event, ctx, app) || handled
    }
}

#[cfg(test)]
#[path = "warpium_blame_column_tests.rs"]
mod tests;
