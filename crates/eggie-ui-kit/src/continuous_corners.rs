//! Continuous rounded rectangles for surfaces that need the flatter Apple-style corner curve.

use gpui::{
    Bounds, CONTINUOUS_CORNER_RADIUS_SCALE, Div, Pixels, Styled, Window, fill, point, px, rgb,
};

/// The standard UI surface; radius helpers also apply to hover, focus, borders and shadows.
pub fn div() -> Div {
    gpui::div().continuous_corners()
}

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

/// Paint a continuous rectangle with an explicit corner extent, including matching nested insets.
pub fn paint_continuous_rect(
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
    window.paint_quad(
        fill(
            Bounds::from_corners(point(px(left), px(top)), point(px(right), px(bottom))),
            rgb(color),
        )
        .corner_radii(px(radius / CONTINUOUS_CORNER_RADIUS_SCALE))
        .continuous_corners(true),
    );
}

/// Cubic approximation of the continuous curve for SVG-backed icon cards.
pub fn svg_rect_path(x: f32, y: f32, width: f32, height: f32, radius: f32) -> String {
    let limit = width.min(height) / 2.;
    let radius = radius.clamp(0., limit);
    let circular = radius >= limit;
    let radius = if circular {
        radius
    } else {
        (radius * CONTINUOUS_CORNER_RADIUS_SCALE).min(limit)
    };
    let coefficient = if circular { 0.5522848 } else { 0.91 };
    let right = x + width;
    let bottom = y + height;
    let mut path = format!("M {} {y} L {} {y}", x + radius, right - radius);
    for (quarter, cx, cy, next_x, next_y) in [
        (0, right - radius, y + radius, right, bottom - radius),
        (1, right - radius, bottom - radius, x + radius, bottom),
        (2, x + radius, bottom - radius, x, y + radius),
        (3, x + radius, y + radius, x + radius, y),
    ] {
        let at = |u, v| corner_point(cx, cy, radius, quarter, u, v);
        let first = at(coefficient, -1.);
        let second = at(1., -coefficient);
        let end = at(1., 0.);
        path.push_str(&format!(
            " C {} {} {} {} {} {} L {next_x} {next_y}",
            f32::from(first.x),
            f32::from(first.y),
            f32::from(second.x),
            f32::from(second.y),
            f32::from(end.x),
            f32::from(end.y)
        ));
    }
    path.push_str(" Z");
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_radius_preserves_the_previous_diagonal_cut() {
        let circular_cut = 1. - 2_f32.powf(-0.5);
        let continuous_cut = CONTINUOUS_CORNER_RADIUS_SCALE * (1. - 2_f32.powf(-0.25));
        assert!((circular_cut - continuous_cut).abs() < 0.000001);
    }

    #[test]
    fn gpu_corner_instance_layout_matches_shader_contract() {
        use gpui::{PolychromeSprite, Quad, Shadow};
        use std::mem::{offset_of, size_of};

        // These offsets also describe the WGSL/HLSL storage arrays; Metal generates its header.
        assert_eq!(size_of::<Quad>(), 168);
        assert_eq!(offset_of!(Quad, corner_shape), 8);
        assert_eq!(offset_of!(Quad, corner_mask), 12);
        assert_eq!(offset_of!(Quad, bounds), 16);
        assert_eq!(size_of::<Shadow>(), 112);
        assert_eq!(offset_of!(Shadow, corner_shape), 108);
        assert_eq!(size_of::<PolychromeSprite>(), 96);
        assert_eq!(offset_of!(PolychromeSprite, corner_shape), 4);
    }

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
