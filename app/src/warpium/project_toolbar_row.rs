//! Warpium: the project toolbar's row, which moves the buttons that don't fit the title bar into
//! the overflow menu.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pathfinder_geometry::vector::{Vector2F, vec2f};
use warpium_project_toolbar::fit_inline_buttons;
use warpui::elements::Point;
use warpui::event::DispatchedEvent;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

/// Lays out the toolbar buttons, then the overflow menu button when some buttons don't fit the
/// width it is given, then the trailing run-configurations widget. Children are vertically
/// centered.
pub(super) struct ToolbarRow {
    buttons: Vec<Box<dyn Element>>,
    overflow_button: Box<dyn Element>,
    trailing: Option<Box<dyn Element>>,
    /// How many leading buttons the last layout showed, shared with the view so the overflow
    /// menu lists the others.
    visible_buttons: Arc<AtomicUsize>,
    visible: usize,
    show_overflow: bool,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl ToolbarRow {
    pub(super) fn new(
        buttons: Vec<Box<dyn Element>>,
        overflow_button: Box<dyn Element>,
        trailing: Option<Box<dyn Element>>,
        visible_buttons: Arc<AtomicUsize>,
    ) -> Self {
        let visible = buttons.len();
        Self {
            buttons,
            overflow_button,
            trailing,
            visible_buttons,
            visible,
            show_overflow: false,
            size: None,
            origin: None,
        }
    }

    fn shown_children(&mut self) -> impl Iterator<Item = &mut Box<dyn Element>> {
        let show_overflow = self.show_overflow;
        self.buttons
            .iter_mut()
            .take(self.visible)
            .chain(show_overflow.then_some(&mut self.overflow_button))
            .chain(self.trailing.as_mut())
    }
}

impl Element for ToolbarRow {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let child_constraint = SizeConstraint::new(
            vec2f(0., constraint.min.y()),
            vec2f(f32::INFINITY, constraint.max.y()),
        );
        let button_widths = self
            .buttons
            .iter_mut()
            .map(|button| button.layout(child_constraint, ctx, app).x())
            .collect::<Vec<_>>();
        let overflow_width = self.overflow_button.layout(child_constraint, ctx, app).x();
        let trailing_width = self.trailing.as_mut().map_or(0., |trailing| {
            trailing.layout(child_constraint, ctx, app).x()
        });

        let fit = fit_inline_buttons(
            &button_widths,
            overflow_width,
            trailing_width,
            constraint.max.x(),
        );
        self.visible = fit.visible;
        self.show_overflow = fit.overflow;
        self.visible_buttons.store(fit.visible, Ordering::Relaxed);

        let (width, height) = self
            .shown_children()
            .filter_map(|child| child.size())
            .fold((0., 0_f32), |(width, height), size| {
                (width + size.x(), height.max(size.y()))
            });
        let size = vec2f(width, height.max(constraint.min.y()));
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        for child in self.shown_children() {
            child.after_layout(ctx, app);
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let height = self.size.map_or(0., |size| size.y());
        let mut x = origin.x();
        for child in self.shown_children() {
            let child_size = child.size().unwrap_or_else(Vector2F::zero);
            child.paint(
                vec2f(x, origin.y() + (height - child_size.y()) / 2.),
                ctx,
                app,
            );
            x += child_size.x();
        }
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
        for child in self.shown_children() {
            handled |= child.dispatch_event(event, ctx, app);
        }
        handled
    }
}
