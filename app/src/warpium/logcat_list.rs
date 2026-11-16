//! Warpium: elements that let the Logcat pane's row list scroll on both axes.

use pathfinder_geometry::vector::Vector2F;
use warpui::elements::new_scrollable::{NewScrollableElement, ScrollableAxis};
use warpui::elements::{Axis, Point, ScrollData, ScrollableElement};
use warpui::event::DispatchedEvent;
use warpui::units::Pixels;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

/// Lets a vertically scrolling list (such as a `UniformList`) be the child of a `NewScrollable`,
/// which can then add horizontal scrolling by clipping. The scrollable handles the wheel on both
/// axes and forwards the vertical part to the list.
pub(super) struct VerticalListAdapter {
    list: Box<dyn ScrollableElement>,
}

impl VerticalListAdapter {
    pub(super) fn new(list: Box<dyn ScrollableElement>) -> Self {
        Self { list }
    }
}

impl Element for VerticalListAdapter {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.list.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.list.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.list.paint(origin, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.list.size()
    }

    fn origin(&self) -> Option<Point> {
        self.list.origin()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.list.dispatch_event(event, ctx, app)
    }
}

impl NewScrollableElement for VerticalListAdapter {
    fn axis(&self) -> ScrollableAxis {
        ScrollableAxis::Vertical
    }

    fn scroll_data(&self, axis: Axis, app: &AppContext) -> Option<ScrollData> {
        match axis {
            Axis::Vertical => self.list.scroll_data(app),
            Axis::Horizontal => None,
        }
    }

    fn scroll(&mut self, delta: Pixels, axis: Axis, ctx: &mut EventContext) {
        match axis {
            Axis::Vertical => self.list.scroll(delta, ctx),
            Axis::Horizontal => {}
        }
    }

    fn axis_should_handle_scroll_wheel(&self, _axis: Axis) -> bool {
        true
    }
}

/// Reports the width its child is given whenever it differs from `known_width`, so the view can
/// re-wrap rows to the pane's width.
pub(super) struct WidthProbe {
    child: Box<dyn Element>,
    known_width: f32,
    width_tx: async_channel::Sender<f32>,
}

impl WidthProbe {
    pub(super) fn new(
        child: Box<dyn Element>,
        known_width: f32,
        width_tx: async_channel::Sender<f32>,
    ) -> Self {
        Self {
            child,
            known_width,
            width_tx,
        }
    }
}

impl Element for WidthProbe {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let width = constraint.max.x();
        if width.is_finite() && (width - self.known_width).abs() >= 0.5 {
            self.known_width = width;
            let _ = self.width_tx.try_send(width);
        }
        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }
}
