//! Warpium: the element drawing one row of the Git Log's commit graph.

use std::rc::Rc;

use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warpium_git::graph::{GraphRow, line_stamps, segment_line};
use warpui::color::ColorU;
use warpui::elements::{CornerRadius, Point, Radius};
use warpui::event::DispatchedEvent;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

const LINE_WIDTH: f32 = 2.;
const NODE_DIAMETER: f32 = 8.;
const HEAD_NODE_DIAMETER: f32 = 10.;
/// Distance between the squares a diagonal is drawn from.
const STAMP_SPACING: f32 = 0.75;

/// Draws a row's lane segments and its commit node. The scene only has rectangles, so diagonals
/// are drawn from small overlapping squares.
pub(super) struct GraphCell {
    row: GraphRow,
    colors: Rc<[ColorU]>,
    lane_width: f32,
    is_head: bool,
    background: ColorU,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl GraphCell {
    pub(super) fn new(
        row: GraphRow,
        colors: Rc<[ColorU]>,
        lane_width: f32,
        is_head: bool,
        background: ColorU,
    ) -> Self {
        Self {
            row,
            colors,
            lane_width,
            is_head,
            background,
            size: None,
            origin: None,
        }
    }

    fn color(&self, index: u16) -> ColorU {
        if self.colors.is_empty() {
            return ColorU::white();
        }
        self.colors[index as usize % self.colors.len()]
    }
}

fn circle(ctx: &mut PaintContext, center: Vector2F, diameter: f32, color: ColorU) {
    let radius = diameter / 2.;
    ctx.scene
        .draw_rect_without_hit_recording(RectF::new(
            center - vec2f(radius, radius),
            vec2f(diameter, diameter),
        ))
        .with_background(color)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(radius)));
}

impl Element for GraphCell {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        _app: &AppContext,
    ) -> Vector2F {
        let size = constraint.max;
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _ctx: &mut AfterLayoutContext, _app: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let Some(size) = self.size else {
            return;
        };
        let height = size.y();
        let half = LINE_WIDTH / 2.;
        for segment in self.row.segments.iter() {
            let color = self.color(segment.color);
            let [from, to] = segment_line(segment, self.lane_width, height);
            if (from.0 - to.0).abs() < f32::EPSILON {
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        origin + vec2f(from.0 - half, from.1),
                        vec2f(LINE_WIDTH, to.1 - from.1),
                    ))
                    .with_background(color);
                continue;
            }
            for (x, y) in line_stamps(from, to, STAMP_SPACING) {
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        origin + vec2f(x - half, y - half),
                        vec2f(LINE_WIDTH, LINE_WIDTH),
                    ))
                    .with_background(color);
            }
        }
        let center = origin + vec2f((self.row.node as f32 + 0.5) * self.lane_width, height / 2.);
        let color = self.color(self.row.color);
        if self.is_head {
            circle(ctx, center, HEAD_NODE_DIAMETER, color);
            circle(
                ctx,
                center,
                HEAD_NODE_DIAMETER - 2. * LINE_WIDTH,
                self.background,
            );
        } else {
            circle(ctx, center, NODE_DIAMETER, color);
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
        _event: &DispatchedEvent,
        _ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        false
    }
}
