//! Continuous rounded rectangles for surfaces that need the flatter Apple-style corner curve.

use gpui::{Bounds, PathBuilder, Pixels, Window, point, px, rgb};

fn corner_point(
    cx: f32,
    cy: f32,
    radius: f32,
    quarter: usize,
    u: f32,
    v: f32,
) -> gpui::Point<Pixels> {
    let (u, v) = match quarter {
        0 => (u, v),
        1 => (-v, u),
        2 => (-u, -v),
        _ => (v, -u),
    };
    point(px(cx + u * radius), px(cy + v * radius))
}

fn add_corner(path: &mut PathBuilder, cx: f32, cy: f32, radius: f32, quarter: usize) {
    let at = |u, v| corner_point(cx, cy, radius, quarter, u, v);
    // A circular quarter uses 0.552 here; 0.91 gives the flatter continuous curve.
    path.cubic_bezier_to(at(1., 0.), at(0.91, -1.), at(1., -0.91));
}

/// Paint a continuous rounded rectangle. Paint nested insets for a matching border or focus ring.
pub(crate) fn paint_continuous_rect(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    inset: f32,
    radius: f32,
    color: u32,
) {
    let left = f32::from(bounds.origin.x) + inset;
    let top = f32::from(bounds.origin.y) + inset;
    let right = f32::from(bounds.origin.x + bounds.size.width) - inset;
    let bottom = f32::from(bounds.origin.y + bounds.size.height) - inset;
    let width = right - left;
    let height = bottom - top;
    if width <= 0. || height <= 0. {
        return;
    }
    let radius = (radius - inset).clamp(0., width.min(height) / 2.);
    let mut path = PathBuilder::fill();
    path.move_to(point(px(left + radius), px(top)));
    path.line_to(point(px(right - radius), px(top)));
    add_corner(&mut path, right - radius, top + radius, radius, 0);
    path.line_to(point(px(right), px(bottom - radius)));
    add_corner(&mut path, right - radius, bottom - radius, radius, 1);
    path.line_to(point(px(left + radius), px(bottom)));
    add_corner(&mut path, left + radius, bottom - radius, radius, 2);
    path.line_to(point(px(left), px(top + radius)));
    add_corner(&mut path, left + radius, top + radius, radius, 3);
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_rotation_keeps_each_quarter_on_its_side() {
        let start = corner_point(0., 0., 10., 0, 0., -1.);
        let right = corner_point(0., 0., 10., 0, 1., 0.);
        let bottom = corner_point(0., 0., 10., 1, 1., 0.);
        assert_eq!((f32::from(start.x), f32::from(start.y)), (0., -10.));
        assert_eq!((f32::from(right.x), f32::from(right.y)), (10., 0.));
        assert_eq!((f32::from(bottom.x), f32::from(bottom.y)), (0., 10.));
    }
}
