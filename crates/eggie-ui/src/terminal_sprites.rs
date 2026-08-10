//! Cell-sized terminal sprites.
//!
//! Terminal drawing characters are geometry, not ordinary prose glyphs.  Drawing them through a
//! font makes their edges depend on fallback selection, hinting, and side bearings, which can leave
//! seams between adjacent cells.  This module follows the same rule as Ghostty's sprite face: these
//! ranges are resolved before the configured font and are rasterized against the exact cell size.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SpriteKind {
    Block,
    Box,
    Braille,
    Geometric,
    MathSymbol,
    Powerline,
    Branch,
    Arrow,
    LegacyDiagonal,
    Legacy,
    SmoothMosaic,
    SegmentedDigit,
    LegacySupplement,
}

impl SpriteKind {
    pub(crate) fn for_char(character: char) -> Option<Self> {
        let cp = character as u32;
        match cp {
            0x2500..=0x257f => Some(Self::Box),
            0x2580..=0x259f => Some(Self::Block),
            0x2800..=0x28ff => Some(Self::Braille),
            0x2320..=0x2321 | 0x239b..=0x23cc | 0x23d0 | 0x23dc..=0x23e1 => Some(Self::MathSymbol),
            0x23f4..=0x23f7 | 0x25e2..=0x25e5 | 0x25f8..=0x25fa | 0x25ff | 0x2bc5..=0x2bc8 => {
                Some(Self::Geometric)
            }
            0x2b60..=0x2b63 | 0x2b66..=0x2b69 => Some(Self::Arrow),
            0xe0b0..=0xe0bf | 0xe0d2 | 0xe0d4 => Some(Self::Powerline),
            0xf5d0..=0xf60d => Some(Self::Branch),
            0x1fb00..=0x1fb3b => Some(Self::Legacy),
            0x1fb3c..=0x1fb67 => Some(Self::SmoothMosaic),
            0x1fba0..=0x1fbae => Some(Self::LegacyDiagonal),
            0x1fb68..=0x1fbaf | 0x1fbbd..=0x1fbbf | 0x1fbce..=0x1fbef => Some(Self::Legacy),
            0x1fbf0..=0x1fbf9 => Some(Self::SegmentedDigit),
            0x1cc1b..=0x1cc1e
            | 0x1cc21..=0x1cc2f
            | 0x1cc30..=0x1cc3f
            | 0x1cd00..=0x1cde5
            | 0x1ce00..=0x1ce01
            | 0x1ce0b..=0x1ce0c
            | 0x1ce16..=0x1ce19
            | 0x1ce51..=0x1ceaf => Some(Self::LegacySupplement),
            _ => None,
        }
    }
}

/// Matches Ghostty's terminal-graphics exemption for minimum contrast. This is deliberately
/// independent from [`SpriteKind::for_char`]: the Powerline block contains glyphs that still use
/// the configured font in Eggie, but their foreground colors must remain untouched.
pub(crate) fn skips_minimum_contrast(character: char) -> bool {
    matches!(
        character as u32,
        0x2500..=0x257f
            | 0x2580..=0x259f
            | 0xe0b0..=0xe0d7
            | 0x1fb00..=0x1fbff
            | 0x1cc00..=0x1cebf
    )
}

pub(crate) fn rasterize(
    character: char,
    width: usize,
    height: usize,
    box_thickness: Option<crate::settings::MetricModifier>,
) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    match SpriteKind::for_char(character)? {
        SpriteKind::Block => Some(rasterize_block(character, width, height)),
        SpriteKind::Box => antialiased(width, height, |width, height| {
            rasterize_box(character, width, height, box_thickness)
        }),
        SpriteKind::Braille => Some(rasterize_braille(character, width, height)),
        SpriteKind::Geometric => antialiased(width, height, |width, height| {
            Some(rasterize_geometric(character, width, height))
        }),
        SpriteKind::MathSymbol => antialiased(width, height, |width, height| {
            Some(rasterize_math_symbol(character, width, height))
        }),
        SpriteKind::Powerline => antialiased(width, height, |width, height| {
            Some(rasterize_powerline(character, width, height))
        }),
        SpriteKind::Branch => antialiased(width, height, |width, height| {
            Some(rasterize_branch(character, width, height))
        }),
        SpriteKind::Arrow => antialiased(width, height, |width, height| {
            Some(rasterize_arrow(character, width, height))
        }),
        SpriteKind::LegacyDiagonal => antialiased(width, height, |width, height| {
            let mut mask = Mask::new(width, height);
            mask.legacy_corner_diagonals((character as u32 - 0x1fba0) as usize);
            Some(mask.pixels)
        }),
        SpriteKind::Legacy => rasterize_legacy(character, width, height),
        SpriteKind::SmoothMosaic => antialiased(width, height, |width, height| {
            Some(rasterize_smooth_mosaic(character, width, height))
        }),
        SpriteKind::SegmentedDigit => antialiased(width, height, |width, height| {
            Some(rasterize_segmented_digit(character, width, height))
        }),
        SpriteKind::LegacySupplement => rasterize_legacy_supplement(character, width, height),
    }
}

const VECTOR_OVERSAMPLE: usize = 4;

fn antialiased(
    width: usize,
    height: usize,
    draw: impl FnOnce(usize, usize) -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    let scale = VECTOR_OVERSAMPLE;
    let source_width = width.checked_mul(scale)?;
    let source_height = height.checked_mul(scale)?;
    let source = draw(source_width, source_height)?;
    if source.len() != source_width * source_height {
        return None;
    }

    let sample_count = (scale * scale) as u32;
    let mut pixels = vec![0; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut coverage = 0u32;
            for sample_y in 0..scale {
                let row = (y * scale + sample_y) * source_width;
                for sample_x in 0..scale {
                    coverage += source[row + x * scale + sample_x] as u32;
                }
            }
            pixels[y * width + x] = ((coverage + sample_count / 2) / sample_count) as u8;
        }
    }
    Some(pixels)
}

fn rasterize_block(character: char, width: usize, height: usize) -> Vec<u8> {
    let mut mask = Mask::new(width, height);
    let eighth_w = |parts: usize| ((width * parts) as f32 / 8.).round() as usize;
    let eighth_h = |parts: usize| ((height * parts) as f32 / 8.).round() as usize;
    match character as u32 {
        0x2580 => mask.fill(0, 0, width, eighth_h(4), 255),
        0x2581..=0x2587 => {
            let parts = (character as u32 - 0x2580) as usize;
            let block_height = eighth_h(parts);
            mask.fill(0, height.saturating_sub(block_height), width, height, 255);
        }
        0x2588 => mask.fill(0, 0, width, height, 255),
        0x2589..=0x258f => {
            let parts = (0x2590 - character as u32) as usize;
            mask.fill(0, 0, eighth_w(parts), height, 255);
        }
        0x2590 => mask.fill(eighth_w(4), 0, width, height, 255),
        // Uniform alpha is intentional.  A font usually turns these into a dither pattern, while
        // terminal sprite renderers treat them as exact light/medium/dark cell shades.
        0x2591 => mask.fill(0, 0, width, height, 0x40),
        0x2592 => mask.fill(0, 0, width, height, 0x80),
        0x2593 => mask.fill(0, 0, width, height, 0xc0),
        0x2594 => mask.fill(0, 0, width, eighth_h(1), 255),
        0x2595 => mask.fill(width.saturating_sub(eighth_w(1)), 0, width, height, 255),
        0x2596..=0x259f => {
            let quadrants = match character as u32 {
                0x2596 => 0b0100,
                0x2597 => 0b1000,
                0x2598 => 0b0001,
                0x2599 => 0b1101,
                0x259a => 0b1001,
                0x259b => 0b0111,
                0x259c => 0b1011,
                0x259d => 0b0010,
                0x259e => 0b0110,
                0x259f => 0b1110,
                _ => 0,
            };
            let mid_x = width / 2;
            let mid_y = height / 2;
            if quadrants & 1 != 0 {
                mask.fill(0, 0, mid_x, mid_y, 255);
            }
            if quadrants & 2 != 0 {
                mask.fill(mid_x, 0, width, mid_y, 255);
            }
            if quadrants & 4 != 0 {
                mask.fill(0, mid_y, mid_x, height, 255);
            }
            if quadrants & 8 != 0 {
                mask.fill(mid_x, mid_y, width, height, 255);
            }
        }
        _ => {}
    }
    mask.pixels
}

fn rasterize_braille(character: char, width: usize, height: usize) -> Vec<u8> {
    let mut mask = Mask::new(width, height);
    let bits = (character as u32 - 0x2800) as u8;
    let dot = (width / 4).min(height / 8).max(1);
    let x_margin = ((width.saturating_sub(dot * 2)) / 3).max(1);
    let y_margin = ((height.saturating_sub(dot * 4)) / 5).max(1);
    let xs = [x_margin, width.saturating_sub(x_margin + dot)];
    let ys = [
        y_margin,
        y_margin * 2 + dot,
        y_margin * 3 + dot * 2,
        height.saturating_sub(y_margin + dot),
    ];
    // Unicode bit order is left 1,2,3; right 4,5,6; left 7; right 8.
    let positions = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    for (bit, (column, row)) in positions.into_iter().enumerate() {
        if bits & (1 << bit) != 0 {
            mask.fill(xs[column], ys[row], xs[column] + dot, ys[row] + dot, 255);
        }
    }
    mask.pixels
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LineStyle {
    #[default]
    None,
    Light,
    Heavy,
    Double,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BoxLines {
    up: LineStyle,
    right: LineStyle,
    down: LineStyle,
    left: LineStyle,
}

impl BoxLines {
    const fn new(up: LineStyle, right: LineStyle, down: LineStyle, left: LineStyle) -> Self {
        Self {
            up,
            right,
            down,
            left,
        }
    }
}

fn box_lines(cp: u32) -> Option<BoxLines> {
    use LineStyle::{Double as D, Heavy as H, Light as L, None as N};
    let lines = match cp {
        0x2500 => BoxLines::new(N, L, N, L),
        0x2501 => BoxLines::new(N, H, N, H),
        0x2502 => BoxLines::new(L, N, L, N),
        0x2503 => BoxLines::new(H, N, H, N),
        0x250c => BoxLines::new(N, L, L, N),
        0x250d => BoxLines::new(N, H, L, N),
        0x250e => BoxLines::new(N, L, H, N),
        0x250f => BoxLines::new(N, H, H, N),
        0x2510 => BoxLines::new(N, N, L, L),
        0x2511 => BoxLines::new(N, N, L, H),
        0x2512 => BoxLines::new(N, N, H, L),
        0x2513 => BoxLines::new(N, N, H, H),
        0x2514 => BoxLines::new(L, L, N, N),
        0x2515 => BoxLines::new(L, H, N, N),
        0x2516 => BoxLines::new(H, L, N, N),
        0x2517 => BoxLines::new(H, H, N, N),
        0x2518 => BoxLines::new(L, N, N, L),
        0x2519 => BoxLines::new(L, N, N, H),
        0x251a => BoxLines::new(H, N, N, L),
        0x251b => BoxLines::new(H, N, N, H),
        0x251c => BoxLines::new(L, L, L, N),
        0x251d => BoxLines::new(L, H, L, N),
        0x251e => BoxLines::new(H, L, L, N),
        0x251f => BoxLines::new(L, L, H, N),
        0x2520 => BoxLines::new(H, L, H, N),
        0x2521 => BoxLines::new(H, H, L, N),
        0x2522 => BoxLines::new(L, H, H, N),
        0x2523 => BoxLines::new(H, H, H, N),
        0x2524 => BoxLines::new(L, N, L, L),
        0x2525 => BoxLines::new(L, N, L, H),
        0x2526 => BoxLines::new(H, N, L, L),
        0x2527 => BoxLines::new(L, N, H, L),
        0x2528 => BoxLines::new(H, N, H, L),
        0x2529 => BoxLines::new(H, N, L, H),
        0x252a => BoxLines::new(L, N, H, H),
        0x252b => BoxLines::new(H, N, H, H),
        0x252c => BoxLines::new(N, L, L, L),
        0x252d => BoxLines::new(N, L, L, H),
        0x252e => BoxLines::new(N, H, L, L),
        0x252f => BoxLines::new(N, H, L, H),
        0x2530 => BoxLines::new(N, L, H, L),
        0x2531 => BoxLines::new(N, L, H, H),
        0x2532 => BoxLines::new(N, H, H, L),
        0x2533 => BoxLines::new(N, H, H, H),
        0x2534 => BoxLines::new(L, L, N, L),
        0x2535 => BoxLines::new(L, L, N, H),
        0x2536 => BoxLines::new(L, H, N, L),
        0x2537 => BoxLines::new(L, H, N, H),
        0x2538 => BoxLines::new(H, L, N, L),
        0x2539 => BoxLines::new(H, L, N, H),
        0x253a => BoxLines::new(H, H, N, L),
        0x253b => BoxLines::new(H, H, N, H),
        0x253c => BoxLines::new(L, L, L, L),
        0x253d => BoxLines::new(L, L, L, H),
        0x253e => BoxLines::new(L, H, L, L),
        0x253f => BoxLines::new(L, H, L, H),
        0x2540 => BoxLines::new(H, L, L, L),
        0x2541 => BoxLines::new(L, L, H, L),
        0x2542 => BoxLines::new(H, L, H, L),
        0x2543 => BoxLines::new(H, L, L, H),
        0x2544 => BoxLines::new(H, H, L, L),
        0x2545 => BoxLines::new(L, L, H, H),
        0x2546 => BoxLines::new(L, H, H, L),
        0x2547 => BoxLines::new(H, H, L, H),
        0x2548 => BoxLines::new(L, H, H, H),
        0x2549 => BoxLines::new(H, L, H, H),
        0x254a => BoxLines::new(H, H, H, L),
        0x254b => BoxLines::new(H, H, H, H),
        0x2550 => BoxLines::new(N, D, N, D),
        0x2551 => BoxLines::new(D, N, D, N),
        0x2552 => BoxLines::new(N, D, L, N),
        0x2553 => BoxLines::new(N, L, D, N),
        0x2554 => BoxLines::new(N, D, D, N),
        0x2555 => BoxLines::new(N, N, L, D),
        0x2556 => BoxLines::new(N, N, D, L),
        0x2557 => BoxLines::new(N, N, D, D),
        0x2558 => BoxLines::new(L, D, N, N),
        0x2559 => BoxLines::new(D, L, N, N),
        0x255a => BoxLines::new(D, D, N, N),
        0x255b => BoxLines::new(L, N, N, D),
        0x255c => BoxLines::new(D, N, N, L),
        0x255d => BoxLines::new(D, N, N, D),
        0x255e => BoxLines::new(L, D, L, N),
        0x255f => BoxLines::new(D, L, D, N),
        0x2560 => BoxLines::new(D, D, D, N),
        0x2561 => BoxLines::new(L, N, L, D),
        0x2562 => BoxLines::new(D, N, D, L),
        0x2563 => BoxLines::new(D, N, D, D),
        0x2564 => BoxLines::new(N, D, L, D),
        0x2565 => BoxLines::new(N, L, D, L),
        0x2566 => BoxLines::new(N, D, D, D),
        0x2567 => BoxLines::new(L, D, N, D),
        0x2568 => BoxLines::new(D, L, N, L),
        0x2569 => BoxLines::new(D, D, N, D),
        0x256a => BoxLines::new(L, D, L, D),
        0x256b => BoxLines::new(D, L, D, L),
        0x256c => BoxLines::new(D, D, D, D),
        0x2574 => BoxLines::new(N, N, N, L),
        0x2575 => BoxLines::new(L, N, N, N),
        0x2576 => BoxLines::new(N, L, N, N),
        0x2577 => BoxLines::new(N, N, L, N),
        0x2578 => BoxLines::new(N, N, N, H),
        0x2579 => BoxLines::new(H, N, N, N),
        0x257a => BoxLines::new(N, H, N, N),
        0x257b => BoxLines::new(N, N, H, N),
        0x257c => BoxLines::new(N, H, N, L),
        0x257d => BoxLines::new(L, N, H, N),
        0x257e => BoxLines::new(N, L, N, H),
        0x257f => BoxLines::new(H, N, L, N),
        _ => return None,
    };
    Some(lines)
}

fn rasterize_box(
    character: char,
    width: usize,
    height: usize,
    box_thickness: Option<crate::settings::MetricModifier>,
) -> Option<Vec<u8>> {
    let cp = character as u32;
    let mut mask = Mask::new(width, height);
    let base_light = (width.min(height) as f32 * 0.11).round().max(1.);
    // `adjust-box-thickness` scales the light stroke; heavy stays 2×. The modifier is device-px,
    // but this runs in oversampled space (`VECTOR_OVERSAMPLE`), so an absolute delta is scaled up.
    let light = match box_thickness {
        Some(modifier) => modifier
            .prescale(VECTOR_OVERSAMPLE as f32)
            .apply(base_light)
            .round()
            .max(1.) as usize,
        None => base_light as usize,
    };
    let heavy = (light * 2).max(2);

    if let Some(lines) = box_lines(cp) {
        mask.box_lines(lines, light, heavy);
        return Some(mask.pixels);
    }

    let mid_x = width / 2;
    let mid_y = height / 2;
    match cp {
        0x2504..=0x250b | 0x254c..=0x254f => {
            let horizontal_dash = matches!(cp, 0x2504 | 0x2505 | 0x2508 | 0x2509 | 0x254c | 0x254d);
            let count = if matches!(cp, 0x2508..=0x250b) {
                4
            } else if matches!(cp, 0x254c..=0x254f) {
                2
            } else {
                3
            };
            let thickness = if cp % 2 == 1 { heavy } else { light };
            let desired_gap = if matches!(cp, 0x254e | 0x254f) {
                heavy
            } else if count == 2 {
                if cp % 2 == 1 { heavy } else { light }
            } else {
                light.max(4 * VECTOR_OVERSAMPLE)
            };
            if horizontal_dash {
                mask.dashed_horizontal(mid_y, thickness, count, desired_gap);
            } else {
                mask.dashed_vertical(mid_x, thickness, count, desired_gap);
            }
        }
        0x256d..=0x2570 => mask.rounded_corner(cp, light),
        0x2571 => mask.diagonal(false, light),
        0x2572 => mask.diagonal(true, light),
        0x2573 => {
            mask.diagonal(false, light);
            mask.diagonal(true, light);
        }
        _ => return None,
    }
    Some(mask.pixels)
}

fn rasterize_geometric(character: char, width: usize, height: usize) -> Vec<u8> {
    let mut mask = Mask::new(width, height);
    let cp = character as u32;
    if matches!(cp, 0x23f4..=0x23f7 | 0x2bc5..=0x2bc8) {
        let direction = match cp {
            0x23f4 | 0x2bc7 => Direction::Left,
            0x23f5 | 0x2bc8 => Direction::Right,
            0x23f6 | 0x2bc5 => Direction::Up,
            _ => Direction::Down,
        };
        let x0 = width as isize / 5;
        let x1 = width as isize - x0;
        let y0 = height as isize / 5;
        let y1 = height as isize - y0;
        let mx = width as isize / 2;
        let my = height as isize / 2;
        let points = match direction {
            Direction::Up => ((mx, y0), (x1, y1), (x0, y1)),
            Direction::Right => ((x1, my), (x0, y1), (x0, y0)),
            Direction::Down => ((mx, y1), (x0, y0), (x1, y0)),
            Direction::Left => ((x0, my), (x1, y0), (x1, y1)),
        };
        mask.triangle(points.0, points.1, points.2, 255);
        return mask.pixels;
    }
    let (corner, outline) = match cp {
        0x25e2 => (Corner::BottomRight, false),
        0x25e3 => (Corner::BottomLeft, false),
        0x25e4 => (Corner::TopLeft, false),
        0x25e5 => (Corner::TopRight, false),
        0x25f8 => (Corner::TopLeft, true),
        0x25f9 => (Corner::TopRight, true),
        0x25fa => (Corner::BottomLeft, true),
        _ => (Corner::BottomRight, true),
    };
    mask.corner_triangle(corner, outline);
    mask.pixels
}

fn rasterize_math_symbol(character: char, width: usize, height: usize) -> Vec<u8> {
    let mut mask = Mask::new(width, height);
    let cp = character as u32;
    let max_x = width.saturating_sub(1) as f32;
    let left_stem = (max_x * 0.28).round() as isize;
    let left_tip = (max_x * 0.78).round() as isize;
    let center_x = (max_x / 2.).round() as isize;
    let height = height as f32;
    let thickness = ((width.min(mask.height) as f32 * 0.11).round() as usize).max(1);

    match cp {
        0x2320..=0x2321 => {
            mask.extensible_integral(cp == 0x2320, center_x, left_tip, height, thickness);
        }
        0x239b..=0x23a0 => {
            let left = cp <= 0x239d;
            let part = ((cp - 0x239b) % 3) as usize;
            let stem_x = if left {
                left_stem
            } else {
                width.saturating_sub(1) as isize - left_stem
            };
            let tip_x = if left {
                left_tip
            } else {
                width.saturating_sub(1) as isize - left_tip
            };
            mask.extensible_hook(part, stem_x, tip_x, height, thickness);
        }
        0x23a1..=0x23a6 => {
            let left = cp <= 0x23a3;
            let part = ((cp - 0x23a1) % 3) as usize;
            let stem_x = if left {
                left_stem
            } else {
                width.saturating_sub(1) as isize - left_stem
            };
            let tip_x = if left {
                width.saturating_sub(1) as isize - left_stem
            } else {
                left_stem
            };
            mask.extensible_square_bracket(part, stem_x, tip_x, height, thickness);
        }
        0x23a7..=0x23ad => {
            let left = cp <= 0x23aa;
            let part = if cp == 0x23aa {
                3
            } else if left {
                (cp - 0x23a7) as usize
            } else {
                (cp - 0x23ab) as usize
            };
            let outer_x = if left {
                width.saturating_sub(1) as isize - left_stem
            } else {
                left_stem
            };
            let inner_x = if left { left_stem } else { left_tip };
            mask.extensible_curly_bracket(part, center_x, outer_x, inner_x, height, thickness);
        }
        0x23ae | 0x23d0 => {
            mask.line((center_x, 0), (center_x, height as isize), thickness);
            mask.vertical_edge_connector(center_x, true, thickness);
            mask.vertical_edge_connector(center_x, false, thickness);
        }
        0x23af => {
            let center_y = (height / 2.).round() as isize;
            mask.line((0, center_y), (width as isize, center_y), thickness);
            mask.horizontal_edge_connector(center_y, true, thickness);
            mask.horizontal_edge_connector(center_y, false, thickness);
        }
        0x23b0..=0x23b1 => {
            mask.extensible_curly_section(
                cp == 0x23b0,
                left_stem,
                width.saturating_sub(1) as isize - left_stem,
                height,
                thickness,
            );
        }
        0x23b2..=0x23b3 => {
            mask.extensible_summation(cp == 0x23b2, left_stem, left_tip, height, thickness);
        }
        0x23b4..=0x23b6 => {
            mask.horizontal_square_bracket(cp, left_stem, left_tip, height, thickness);
        }
        0x23b7 => {
            mask.radical_bottom(left_stem, left_tip, height, thickness);
        }
        0x23b8..=0x23b9 => {
            let x = if cp == 0x23b8 {
                left_stem
            } else {
                width.saturating_sub(1) as isize - left_stem
            };
            mask.line((x, 0), (x, height as isize), thickness);
            mask.vertical_edge_connector(x, true, thickness);
            mask.vertical_edge_connector(x, false, thickness);
        }
        0x23ba..=0x23bd => {
            let scan_line = [1., 3., 7., 9.][(cp - 0x23ba) as usize];
            let y = ((scan_line - 1.) / 8. * (height - 1.)).round() as isize;
            mask.line((0, y), (width as isize, y), thickness);
            mask.horizontal_edge_connector(y, true, thickness);
            mask.horizontal_edge_connector(y, false, thickness);
        }
        0x23be..=0x23cc => {
            mask.dentistry_symbol(cp, center_x, height, thickness);
        }
        0x23dc..=0x23e1 => {
            mask.horizontal_math_symbol(cp, height, thickness);
        }
        _ => unreachable!(),
    }

    mask.pixels
}

fn rasterize_powerline(character: char, width: usize, height: usize) -> Vec<u8> {
    let mut mask = Mask::new(width, height);
    let cp = character as u32;
    let light = ((width.min(height) as f32 * 0.11).round() as usize).max(1);
    match cp {
        0xe0b0 => mask.triangle(
            (0, 0),
            (width as isize, height as isize / 2),
            (0, height as isize),
            255,
        ),
        0xe0b1 => mask.chevron(false, light),
        0xe0b2 => mask.triangle(
            (width as isize, 0),
            (0, height as isize / 2),
            (width as isize, height as isize),
            255,
        ),
        0xe0b3 => mask.chevron(true, light),
        0xe0b4..=0xe0b7 => mask.powerline_round(cp, light),
        0xe0b8 => mask.corner_triangle(Corner::BottomRight, false),
        0xe0b9 => mask.diagonal(false, light),
        0xe0ba => mask.corner_triangle(Corner::BottomLeft, false),
        0xe0bb => mask.diagonal(true, light),
        0xe0bc => mask.corner_triangle(Corner::TopLeft, false),
        0xe0bd => mask.diagonal(true, light),
        0xe0be => mask.corner_triangle(Corner::TopRight, false),
        0xe0bf => mask.diagonal(false, light),
        0xe0d2 | 0xe0d4 => mask.powerline_hourglass(cp == 0xe0d4, light),
        _ => {}
    }
    mask.pixels
}

fn rasterize_branch(character: char, width: usize, height: usize) -> Vec<u8> {
    let mut mask = Mask::new(width, height);
    let cp = character as u32;
    let light = ((width.min(height) as f32 * 0.11).round() as usize).max(1);
    match cp {
        0xf5d0 => mask.junction(false, true, false, true, light, light),
        0xf5d1 => mask.junction(true, false, true, false, light, light),
        0xf5d2..=0xf5d5 => mask.fading_line(cp, light),
        0xf5d6..=0xf5d9 => {
            let corner = match cp {
                0xf5d6 => Corner::BottomRight,
                0xf5d7 => Corner::BottomLeft,
                0xf5d8 => Corner::TopRight,
                _ => Corner::TopLeft,
            };
            mask.rounded_corner_for_branch(corner, light);
        }
        0xf5da..=0xf5ed => mask.branch_arc_composite(cp, light),
        0xf5ee | 0xf5ef => mask.branch_node(0, cp == 0xf5ee, light),
        0xf5f0..=0xf60d => {
            const CONNECTIONS: [u8; 15] = [
                0b0010, 0b1000, 0b1010, 0b0100, 0b0001, 0b0101, 0b0110, 0b1100, 0b0011, 0b1001,
                0b0111, 0b1101, 0b1110, 0b1011, 0b1111,
            ];
            let index = ((cp - 0xf5f0) / 2) as usize;
            mask.branch_node(CONNECTIONS[index], cp.is_multiple_of(2), light);
        }
        _ => {}
    }
    mask.pixels
}

fn rasterize_arrow(character: char, width: usize, height: usize) -> Vec<u8> {
    let direction: (f32, f32) = match character as u32 {
        0x2b60 => (-1., 0.),
        0x2b61 => (0., -1.),
        0x2b62 => (1., 0.),
        0x2b63 => (0., 1.),
        0x2b66 => (-1., -1.),
        0x2b67 => (1., -1.),
        0x2b68 => (1., 1.),
        _ => (-1., 1.),
    };
    let length = (direction.0 * direction.0 + direction.1 * direction.1).sqrt();
    let dx = direction.0 / length;
    let dy = direction.1 / length;
    let perpendicular = (-dy, dx);
    let center = (width as f32 / 2., height as f32 / 2.);
    let span = (width.min(height) as f32 * 0.4).max(1.);
    let head_length = (width.min(height) as f32 * 0.36).max(2.);
    let head_width = (width.min(height) as f32 * 0.58).max(2.);
    let tip = (center.0 + dx * span, center.1 + dy * span);
    let tail = (center.0 - dx * span, center.1 - dy * span);
    let base = (tip.0 - dx * head_length, tip.1 - dy * head_length);
    let left = (
        base.0 + perpendicular.0 * head_width / 2.,
        base.1 + perpendicular.1 * head_width / 2.,
    );
    let right = (
        base.0 - perpendicular.0 * head_width / 2.,
        base.1 - perpendicular.1 * head_width / 2.,
    );
    let point = |point: (f32, f32)| (point.0.round() as isize, point.1.round() as isize);

    let mut mask = Mask::new(width, height);
    mask.line(point(tail), point(base), (width.min(height) / 9).max(1));
    mask.triangle(point(tip), point(left), point(right), 255);
    mask.pixels
}

fn rasterize_smooth_mosaic(character: char, width: usize, height: usize) -> Vec<u8> {
    // The ten bits select perimeter vertices clockwise from the top-left. This is the same
    // cell-relative geometry used by modern terminal sprite faces, so every filled edge reaches
    // the exact cell boundary instead of inheriting a font's side bearings.
    const VERTICES: [u16; 44] = [
        0b0000011100,
        0b0000101100,
        0b0000011010,
        0b0000101010,
        0b0000011001,
        0b1100101010,
        0b0100101010,
        0b1100101100,
        0b0100101100,
        0b1100101000,
        0b0010101100,
        0b0001110000,
        0b0001101000,
        0b0010110000,
        0b0010101000,
        0b0100110000,
        0b1010101001,
        0b0010101001,
        0b1001101001,
        0b0001101001,
        0b1000101001,
        0b0001101010,
        0b0100110101,
        0b0100100101,
        0b0100110011,
        0b0100100011,
        0b0100110001,
        0b1000000011,
        0b0100000011,
        0b1000000101,
        0b0100000101,
        0b1000001001,
        0b0110000101,
        0b0101011001,
        0b0101001001,
        0b0110011001,
        0b0110001001,
        0b0100011001,
        0b1110000000,
        0b0110000001,
        0b1101000000,
        0b0101000001,
        0b1100100000,
        0b0101000011,
    ];
    let bits = VERTICES[(character as u32 - 0x1fb3c) as usize];
    let third = height as f32 / 3.;
    let perimeter = [
        (0., 0.),
        (0., third),
        (0., third * 2.),
        (0., height as f32),
        (width as f32 / 2., height as f32),
        (width as f32, height as f32),
        (width as f32, third * 2.),
        (width as f32, third),
        (width as f32, 0.),
        (width as f32 / 2., 0.),
    ];
    let points = perimeter
        .into_iter()
        .enumerate()
        .filter_map(|(index, point)| (bits & (1 << index) != 0).then_some(point))
        .collect::<Vec<_>>();
    let mut mask = Mask::new(width, height);
    mask.polygon(&points, 255);
    mask.pixels
}

fn rasterize_legacy(character: char, width: usize, height: usize) -> Option<Vec<u8>> {
    let cp = character as u32;
    let mut mask = Mask::new(width, height);
    match cp {
        // Sextants. Unicode omits the all-empty/all-full and a few patterns already represented by
        // block elements, so the index is expanded before reading its six coverage bits.
        0x1fb00..=0x1fb3b => {
            let index = cp - 0x1fb00;
            let bits = index + index / 0x14 + 1;
            let mid_x = width / 2;
            let third_1 = ((height as f32 / 3.).round() as usize).min(height);
            let third_2 = ((height as f32 * 2. / 3.).round() as usize).min(height);
            let cells = [
                (0, 0, mid_x, third_1),
                (mid_x, 0, width, third_1),
                (0, third_1, mid_x, third_2),
                (mid_x, third_1, width, third_2),
                (0, third_2, mid_x, height),
                (mid_x, third_2, width, height),
            ];
            for (bit, (x0, y0, x1, y1)) in cells.into_iter().enumerate() {
                if bits & (1 << bit) != 0 {
                    mask.fill(x0, y0, x1, y1, 255);
                }
            }
        }
        0x1fb68..=0x1fb6f => {
            let edge = (cp - 0x1fb68) % 4;
            mask.edge_triangle(edge, 255);
            if cp < 0x1fb6c {
                for value in &mut mask.pixels {
                    *value = 255 - *value;
                }
            }
        }
        // Interior one-eighth stripes.
        0x1fb70..=0x1fb75 => {
            let stripe = (cp - 0x1fb70 + 1) as usize;
            let x0 = (width as f32 * stripe as f32 / 8.).round() as usize;
            let x1 = (width as f32 * (stripe + 1) as f32 / 8.).round() as usize;
            mask.fill(x0, 0, x1, height, 255);
        }
        0x1fb76..=0x1fb7b => {
            let stripe = (cp - 0x1fb76 + 1) as usize;
            let y0 = (height as f32 * stripe as f32 / 8.).round() as usize;
            let y1 = (height as f32 * (stripe + 1) as f32 / 8.).round() as usize;
            mask.fill(0, y0, width, y1, 255);
        }
        0x1fb7c..=0x1fb80 => {
            let eighth_w = (width as f32 / 8.).round().max(1.) as usize;
            let eighth_h = (height as f32 / 8.).round().max(1.) as usize;
            match cp {
                0x1fb7c => {
                    mask.fill(0, 0, eighth_w, height, 255);
                    mask.fill(0, height.saturating_sub(eighth_h), width, height, 255);
                }
                0x1fb7d => {
                    mask.fill(0, 0, eighth_w, height, 255);
                    mask.fill(0, 0, width, eighth_h, 255);
                }
                0x1fb7e => {
                    mask.fill(width.saturating_sub(eighth_w), 0, width, height, 255);
                    mask.fill(0, 0, width, eighth_h, 255);
                }
                0x1fb7f => {
                    mask.fill(width.saturating_sub(eighth_w), 0, width, height, 255);
                    mask.fill(0, height.saturating_sub(eighth_h), width, height, 255);
                }
                _ => {
                    mask.fill(0, 0, width, eighth_h, 255);
                    mask.fill(0, height.saturating_sub(eighth_h), width, height, 255);
                }
            }
        }
        0x1fb81 => {
            for eighth in [0usize, 2, 4, 7] {
                let y0 = (height as f32 * eighth as f32 / 8.).round() as usize;
                let y1 = (height as f32 * (eighth + 1) as f32 / 8.).round() as usize;
                mask.fill(0, y0, width, y1, 255);
            }
        }
        0x1fb82..=0x1fb86 => {
            let eighths = [2usize, 3, 5, 6, 7][(cp - 0x1fb82) as usize];
            let bottom = (height as f32 * eighths as f32 / 8.).round() as usize;
            mask.fill(0, 0, width, bottom, 255);
        }
        0x1fb87..=0x1fb8b => {
            let eighths = [2usize, 3, 5, 6, 7][(cp - 0x1fb87) as usize];
            let left = width.saturating_sub((width as f32 * eighths as f32 / 8.).round() as usize);
            mask.fill(left, 0, width, height, 255);
        }
        0x1fb8c..=0x1fb90 => match cp {
            0x1fb8c => mask.fill(0, 0, width / 2, height, 0x80),
            0x1fb8d => mask.fill(width / 2, 0, width, height, 0x80),
            0x1fb8e => mask.fill(0, 0, width, height / 2, 0x80),
            0x1fb8f => mask.fill(0, height / 2, width, height, 0x80),
            _ => mask.fill(0, 0, width, height, 0x80),
        },
        0x1fb91 => {
            mask.fill(0, 0, width, height, 0x80);
            mask.fill(0, 0, width, height / 2, 255);
        }
        0x1fb92 => {
            mask.fill(0, 0, width, height, 0x80);
            mask.fill(0, height / 2, width, height, 255);
        }
        0x1fb93 => {}
        0x1fb94 => {
            mask.fill(0, 0, width, height, 0x80);
            mask.fill(width / 2, 0, width, height, 255);
        }
        0x1fb95..=0x1fb96 => {
            let inverse = cp == 0x1fb96;
            for y in 0..height {
                for x in 0..width {
                    if ((x + y) % 2 == 0) != inverse {
                        mask.pixels[y * width + x] = 255;
                    }
                }
            }
        }
        0x1fb97 => {
            mask.fill(0, height / 4, width, height / 2, 255);
            mask.fill(0, height * 3 / 4, width, height, 255);
        }
        0x1fb98..=0x1fb99 => {
            let descending = cp == 0x1fb98;
            let thickness = (width.min(height) / 8).max(1);
            let stride = (width / 2).max(thickness * 2).max(1);
            let extent = width + height;
            for offset in (0..=extent * 2).step_by(stride) {
                let shift = offset as isize - extent as isize;
                let (start, end) = if descending {
                    ((shift, 0), (shift + width as isize, height as isize))
                } else {
                    ((width as isize - shift, 0), (-shift, height as isize))
                };
                mask.line(start, end, thickness);
            }
        }
        0x1fb9a..=0x1fb9b => {
            if cp == 0x1fb9a {
                mask.edge_triangle(1, 255);
                mask.edge_triangle(3, 255);
            } else {
                mask.edge_triangle(0, 255);
                mask.edge_triangle(2, 255);
            }
        }
        0x1fb9c..=0x1fb9f => {
            let corner = [
                Corner::TopLeft,
                Corner::TopRight,
                Corner::BottomRight,
                Corner::BottomLeft,
            ][(cp - 0x1fb9c) as usize];
            mask.corner_triangle(corner, false);
            for value in &mut mask.pixels {
                *value = ((*value as u16 * 0x80) / 0xff) as u8;
            }
        }
        0x1fba0..=0x1fbae => mask.legacy_corner_diagonals((cp - 0x1fba0) as usize),
        0x1fbaf => mask.junction(true, true, true, true, 2, 1),
        0x1fbbd => {
            mask.diagonal(false, 1);
            mask.diagonal(true, 1);
            for value in &mut mask.pixels {
                *value = 255 - *value;
            }
        }
        0x1fbbe..=0x1fbbf => {
            mask.legacy_corner_diagonals(if cp == 0x1fbbe { 3 } else { 14 });
            for value in &mut mask.pixels {
                *value = 255 - *value;
            }
        }
        0x1fbce => mask.fill(0, 0, (width as f32 * 2. / 3.).round() as usize, height, 255),
        0x1fbcf => mask.fill(0, 0, (width as f32 / 3.).round() as usize, height, 255),
        0x1fbd0..=0x1fbdf => mask.legacy_cell_diagonal((cp - 0x1fbd0) as usize),
        0x1fbe0..=0x1fbef => mask.legacy_circle_or_square((cp - 0x1fbe0) as usize),
        _ => return None,
    }
    Some(mask.pixels)
}

fn rasterize_segmented_digit(character: char, width: usize, height: usize) -> Vec<u8> {
    // U+1FBF0..U+1FBF9 are absent from macOS's system fallback collection. Keep them
    // independent from the configured font, just like the rest of the terminal graphics face.
    // Bit order is top, upper-right, lower-right, bottom, lower-left, upper-left, middle.
    const SEGMENTS: [u8; 10] = [
        0b011_1111, // 0
        0b000_0110, // 1
        0b101_1011, // 2
        0b100_1111, // 3
        0b110_0110, // 4
        0b110_1101, // 5
        0b111_1101, // 6
        0b000_0111, // 7
        0b111_1111, // 8
        0b110_1111, // 9
    ];

    let digit = (character as u32 - 0x1fbf0) as usize;
    let segments = SEGMENTS[digit.min(SEGMENTS.len() - 1)];
    let mut mask = Mask::new(width, height);
    let stroke = (width.min(height) / 7).max(1);
    let margin_x = (width / 5).max(stroke / 2);
    let margin_y = (height / 10).max(stroke / 2);
    let left = margin_x.min(width.saturating_sub(1));
    let right = width.saturating_sub(margin_x).max(left + 1).min(width);
    let top = margin_y.min(height.saturating_sub(1));
    let bottom = height.saturating_sub(margin_y).max(top + 1).min(height);
    let middle = (top + bottom) / 2;

    let horizontal = |mask: &mut Mask, y: usize| {
        mask.fill(
            left,
            y.saturating_sub(stroke / 2),
            right,
            (y + stroke.div_ceil(2)).min(height),
            255,
        );
    };
    let vertical = |mask: &mut Mask, x: usize, y0: usize, y1: usize| {
        mask.fill(
            x.saturating_sub(stroke / 2),
            y0,
            (x + stroke.div_ceil(2)).min(width),
            y1,
            255,
        );
    };

    if segments & (1 << 0) != 0 {
        horizontal(&mut mask, top);
    }
    if segments & (1 << 1) != 0 {
        vertical(&mut mask, right.saturating_sub(1), top, middle + 1);
    }
    if segments & (1 << 2) != 0 {
        vertical(&mut mask, right.saturating_sub(1), middle, bottom);
    }
    if segments & (1 << 3) != 0 {
        horizontal(&mut mask, bottom.saturating_sub(1));
    }
    if segments & (1 << 4) != 0 {
        vertical(&mut mask, left, middle, bottom);
    }
    if segments & (1 << 5) != 0 {
        vertical(&mut mask, left, top, middle + 1);
    }
    if segments & (1 << 6) != 0 {
        horizontal(&mut mask, middle);
    }
    mask.pixels
}

// Unicode 16 assigns codepoints only to octant combinations that did not already have an
// equivalent Block Elements character. The values are the occupied octants encoded as bits 1..8,
// in codepoint order from U+1CD00 through U+1CDE5. Keeping the normative mapping explicit makes
// rasterization a single indexed lookup instead of a font fallback on systems without Unicode 16
// fonts.
const OCTANT_BITS: [u8; 230] = [
    0x04, 0x06, 0x07, 0x08, 0x09, 0x0b, 0x0c, 0x0d, 0x0e, 0x10, 0x11, 0x12, 0x13, 0x15, 0x16, 0x17,
    0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
    0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a,
    0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x51, 0x52, 0x53, 0x54, 0x56, 0x57, 0x58, 0x59, 0x5b, 0x5c, 0x5d,
    0x5e, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e,
    0x6f, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x7b, 0x7c, 0x7d, 0x7e,
    0x7f, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f,
    0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x9f,
    0xa1, 0xa2, 0xa3, 0xa4, 0xa6, 0xa7, 0xa8, 0xa9, 0xab, 0xac, 0xad, 0xae, 0xb0, 0xb1, 0xb2, 0xb3,
    0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xbc, 0xbd, 0xbe, 0xbf, 0xc1, 0xc2, 0xc3, 0xc4,
    0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xcb, 0xcc, 0xcd, 0xce, 0xcf, 0xd0, 0xd1, 0xd2, 0xd3, 0xd4,
    0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xdb, 0xdc, 0xdd, 0xde, 0xdf, 0xe0, 0xe1, 0xe2, 0xe3, 0xe4,
    0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xeb, 0xec, 0xed, 0xee, 0xef, 0xf1, 0xf2, 0xf3, 0xf4, 0xf6,
    0xf7, 0xf8, 0xf9, 0xfb, 0xfd, 0xfe,
];

fn rasterize_legacy_supplement(character: char, width: usize, height: usize) -> Option<Vec<u8>> {
    let cp = character as u32;
    let mut mask = Mask::new(width, height);
    match cp {
        0x1cd00..=0x1cde5 => {
            let bits = OCTANT_BITS[(cp - 0x1cd00) as usize];
            for octant in 0..8 {
                if bits & (1 << octant) == 0 {
                    continue;
                }
                let column = octant % 2;
                let row = octant / 2;
                mask.fill(
                    column * width / 2,
                    row * height / 4,
                    (column + 1) * width / 2,
                    (row + 1) * height / 4,
                    255,
                );
            }
        }
        // Separated block quadrants have a fixed one-cell margin and centre gap.
        0x1cc21..=0x1cc2f => {
            let bits = (cp - 0x1cc20) as u8;
            let gap = (width / 12).max(1);
            let mid_x = width / 2;
            let mid_y = height / 2;
            let cells = [
                (
                    gap,
                    gap,
                    mid_x.saturating_sub(gap),
                    mid_y.saturating_sub(gap),
                ),
                (
                    mid_x + gap,
                    gap,
                    width.saturating_sub(gap),
                    mid_y.saturating_sub(gap),
                ),
                (
                    gap,
                    mid_y + gap,
                    mid_x.saturating_sub(gap),
                    height.saturating_sub(gap),
                ),
                (
                    mid_x + gap,
                    mid_y + gap,
                    width.saturating_sub(gap),
                    height.saturating_sub(gap),
                ),
            ];
            for (bit, (x0, y0, x1, y1)) in cells.into_iter().enumerate() {
                if bits & (1 << bit) != 0 {
                    mask.fill(x0, y0, x1, y1, 255);
                }
            }
        }
        _ => return None,
    }
    Some(mask.pixels)
}

#[derive(Clone, Copy)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy)]
enum Direction {
    Up,
    Right,
    Down,
    Left,
}

struct Mask {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Mask {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height],
        }
    }

    fn fill(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, alpha: u8) {
        for y in y0.min(self.height)..y1.min(self.height) {
            for x in x0.min(self.width)..x1.min(self.width) {
                self.pixels[y * self.width + x] = alpha;
            }
        }
    }

    fn junction(
        &mut self,
        up: bool,
        right: bool,
        down: bool,
        left: bool,
        vertical: usize,
        horizontal: usize,
    ) {
        let mx = self.width / 2;
        let my = self.height / 2;
        let vx0 = mx.saturating_sub(vertical / 2);
        let vx1 = (mx + vertical.div_ceil(2)).min(self.width);
        let hy0 = my.saturating_sub(horizontal / 2);
        let hy1 = (my + horizontal.div_ceil(2)).min(self.height);
        if up {
            self.fill(vx0, 0, vx1, my + 1, 255);
        }
        if down {
            self.fill(vx0, my, vx1, self.height, 255);
        }
        if left {
            self.fill(0, hy0, mx + 1, hy1, 255);
        }
        if right {
            self.fill(mx, hy0, self.width, hy1, 255);
        }
    }

    fn box_lines(&mut self, lines: BoxLines, light: usize, heavy: usize) {
        let h_light_top = self.height.saturating_sub(light) / 2;
        let h_light_bottom = h_light_top + light;
        let h_heavy_top = self.height.saturating_sub(heavy) / 2;
        let h_heavy_bottom = h_heavy_top + heavy;
        let h_double_top = h_light_top.saturating_sub(light);
        let h_double_bottom = (h_light_bottom + light).min(self.height);

        let v_light_left = self.width.saturating_sub(light) / 2;
        let v_light_right = v_light_left + light;
        let v_heavy_left = self.width.saturating_sub(heavy) / 2;
        let v_heavy_right = v_heavy_left + heavy;
        let v_double_left = v_light_left.saturating_sub(light);
        let v_double_right = (v_light_right + light).min(self.width);

        let up_bottom = if lines.left == LineStyle::Heavy || lines.right == LineStyle::Heavy {
            h_heavy_bottom
        } else if lines.left != lines.right || lines.down == lines.up {
            if lines.left == LineStyle::Double || lines.right == LineStyle::Double {
                h_double_bottom
            } else {
                h_light_bottom
            }
        } else if lines.left == LineStyle::None && lines.right == LineStyle::None {
            h_light_bottom
        } else {
            h_light_top
        };
        let down_top = if lines.left == LineStyle::Heavy || lines.right == LineStyle::Heavy {
            h_heavy_top
        } else if lines.left != lines.right || lines.up == lines.down {
            if lines.left == LineStyle::Double || lines.right == LineStyle::Double {
                h_double_top
            } else {
                h_light_top
            }
        } else if lines.left == LineStyle::None && lines.right == LineStyle::None {
            h_light_top
        } else {
            h_light_bottom
        };
        let left_right = if lines.up == LineStyle::Heavy || lines.down == LineStyle::Heavy {
            v_heavy_right
        } else if lines.up != lines.down || lines.left == lines.right {
            if lines.up == LineStyle::Double || lines.down == LineStyle::Double {
                v_double_right
            } else {
                v_light_right
            }
        } else if lines.up == LineStyle::None && lines.down == LineStyle::None {
            v_light_right
        } else {
            v_light_left
        };
        let right_left = if lines.up == LineStyle::Heavy || lines.down == LineStyle::Heavy {
            v_heavy_left
        } else if lines.up != lines.down || lines.right == lines.left {
            if lines.up == LineStyle::Double || lines.down == LineStyle::Double {
                v_double_left
            } else {
                v_light_left
            }
        } else if lines.up == LineStyle::None && lines.down == LineStyle::None {
            v_light_left
        } else {
            v_light_right
        };

        match lines.up {
            LineStyle::None => {}
            LineStyle::Light => self.fill(v_light_left, 0, v_light_right, up_bottom, 255),
            LineStyle::Heavy => self.fill(v_heavy_left, 0, v_heavy_right, up_bottom, 255),
            LineStyle::Double => {
                let left_bottom = if lines.left == LineStyle::Double {
                    h_light_top
                } else {
                    up_bottom
                };
                let right_bottom = if lines.right == LineStyle::Double {
                    h_light_top
                } else {
                    up_bottom
                };
                self.fill(v_double_left, 0, v_light_left, left_bottom, 255);
                self.fill(v_light_right, 0, v_double_right, right_bottom, 255);
            }
        }

        match lines.right {
            LineStyle::None => {}
            LineStyle::Light => self.fill(right_left, h_light_top, self.width, h_light_bottom, 255),
            LineStyle::Heavy => self.fill(right_left, h_heavy_top, self.width, h_heavy_bottom, 255),
            LineStyle::Double => {
                let top_left = if lines.up == LineStyle::Double {
                    v_light_right
                } else {
                    right_left
                };
                let bottom_left = if lines.down == LineStyle::Double {
                    v_light_right
                } else {
                    right_left
                };
                self.fill(top_left, h_double_top, self.width, h_light_top, 255);
                self.fill(
                    bottom_left,
                    h_light_bottom,
                    self.width,
                    h_double_bottom,
                    255,
                );
            }
        }

        match lines.down {
            LineStyle::None => {}
            LineStyle::Light => self.fill(v_light_left, down_top, v_light_right, self.height, 255),
            LineStyle::Heavy => self.fill(v_heavy_left, down_top, v_heavy_right, self.height, 255),
            LineStyle::Double => {
                let left_top = if lines.left == LineStyle::Double {
                    h_light_bottom
                } else {
                    down_top
                };
                let right_top = if lines.right == LineStyle::Double {
                    h_light_bottom
                } else {
                    down_top
                };
                self.fill(v_double_left, left_top, v_light_left, self.height, 255);
                self.fill(v_light_right, right_top, v_double_right, self.height, 255);
            }
        }

        match lines.left {
            LineStyle::None => {}
            LineStyle::Light => self.fill(0, h_light_top, left_right, h_light_bottom, 255),
            LineStyle::Heavy => self.fill(0, h_heavy_top, left_right, h_heavy_bottom, 255),
            LineStyle::Double => {
                let top_right = if lines.up == LineStyle::Double {
                    v_light_left
                } else {
                    left_right
                };
                let bottom_right = if lines.down == LineStyle::Double {
                    v_light_left
                } else {
                    left_right
                };
                self.fill(0, h_double_top, top_right, h_light_top, 255);
                self.fill(0, h_light_bottom, bottom_right, h_double_bottom, 255);
            }
        }
    }

    fn dashed_horizontal(&mut self, y: usize, thickness: usize, count: usize, desired_gap: usize) {
        if self.width < count * 2 {
            self.fill(
                0,
                y.saturating_sub(thickness / 2),
                self.width,
                y + thickness.div_ceil(2),
                255,
            );
            return;
        }
        let gap = desired_gap.min(self.width / (2 * count)).max(1);
        let total_dash = self.width - count * gap;
        let dash = total_dash / count;
        let mut extra = total_dash % count;
        let mut x = gap / 2;
        for _ in 0..count {
            let width = dash + usize::from(extra > 0);
            extra = extra.saturating_sub(1);
            self.fill(
                x,
                y.saturating_sub(thickness / 2),
                x + width,
                y + thickness.div_ceil(2),
                255,
            );
            x += width + gap;
        }
    }

    fn dashed_vertical(&mut self, x: usize, thickness: usize, count: usize, desired_gap: usize) {
        if self.height < count * 2 {
            self.fill(
                x.saturating_sub(thickness / 2),
                0,
                x + thickness.div_ceil(2),
                self.height,
                255,
            );
            return;
        }
        let gap = desired_gap.min(self.height / (2 * count)).max(1);
        let total_dash = self.height - count * gap;
        let dash = total_dash / count;
        let mut extra = total_dash % count;
        let mut y = 0;
        for _ in 0..count {
            let height = dash + usize::from(extra > 0);
            extra = extra.saturating_sub(1);
            self.fill(
                x.saturating_sub(thickness / 2),
                y,
                x + thickness.div_ceil(2),
                y + height,
                255,
            );
            y += height + gap;
        }
    }

    fn rounded_corner(&mut self, cp: u32, thickness: usize) {
        let corner = match cp {
            0x256d => Corner::TopLeft,
            0x256e => Corner::TopRight,
            0x256f => Corner::BottomRight,
            _ => Corner::BottomLeft,
        };
        // Match Ghostty's cell sprite geometry: use a circular Bézier arc centered in the cell,
        // with straight legs extending to the cell edges. This keeps a tall terminal cell from
        // stretching the curve into an ellipse while still joining adjacent `─` and `│` sprites.
        let center_x = (self.width.saturating_sub(thickness) / 2) as f32 + thickness as f32 / 2.;
        let center_y = (self.height.saturating_sub(thickness) / 2) as f32 + thickness as f32 / 2.;
        let radius = self.width.min(self.height) as f32 / 2.;
        let control = radius * 0.25;
        let width = self.width as f32;
        let height = self.height as f32;

        let (start, curve_start, control_1, control_2, curve_end, end) = match corner {
            // ╭: bottom edge -> bottom-right quarter arc -> right edge.
            Corner::TopLeft => (
                (center_x, height),
                (center_x, center_y + radius),
                (center_x, center_y + control),
                (center_x + control, center_y),
                (center_x + radius, center_y),
                (width, center_y),
            ),
            // ╮: bottom edge -> bottom-left quarter arc -> left edge.
            Corner::TopRight => (
                (center_x, height),
                (center_x, center_y + radius),
                (center_x, center_y + control),
                (center_x - control, center_y),
                (center_x - radius, center_y),
                (0., center_y),
            ),
            // ╯: top edge -> top-left quarter arc -> left edge.
            Corner::BottomRight => (
                (center_x, 0.),
                (center_x, center_y - radius),
                (center_x, center_y - control),
                (center_x - control, center_y),
                (center_x - radius, center_y),
                (0., center_y),
            ),
            // ╰: top edge -> top-right quarter arc -> right edge.
            Corner::BottomLeft => (
                (center_x, 0.),
                (center_x, center_y - radius),
                (center_x, center_y - control),
                (center_x + control, center_y),
                (center_x + radius, center_y),
                (width, center_y),
            ),
        };

        let pixel_point = |point: (f32, f32)| (point.0.round() as isize, point.1.round() as isize);
        self.line(pixel_point(start), pixel_point(curve_start), thickness);
        let steps = (self.width.max(self.height) * 2).max(16);
        let mut previous = curve_start;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let one_minus_t = 1. - t;
            let point = (
                one_minus_t.powi(3) * curve_start.0
                    + 3. * one_minus_t.powi(2) * t * control_1.0
                    + 3. * one_minus_t * t.powi(2) * control_2.0
                    + t.powi(3) * curve_end.0,
                one_minus_t.powi(3) * curve_start.1
                    + 3. * one_minus_t.powi(2) * t * control_1.1
                    + 3. * one_minus_t * t.powi(2) * control_2.1
                    + t.powi(3) * curve_end.1,
            );
            self.line(pixel_point(previous), pixel_point(point), thickness);
            previous = point;
        }
        self.line(pixel_point(curve_end), pixel_point(end), thickness);

        let vx0 = self.width.saturating_sub(thickness) / 2;
        let vx1 = vx0 + thickness;
        let hy0 = self.height.saturating_sub(thickness) / 2;
        let hy1 = hy0 + thickness;
        let right = matches!(corner, Corner::TopLeft | Corner::BottomLeft);
        let bottom = matches!(corner, Corner::TopLeft | Corner::TopRight);
        let edge_width = VECTOR_OVERSAMPLE.min(self.width);
        let edge_height = VECTOR_OVERSAMPLE.min(self.height);
        let edge_x = if right {
            self.width - edge_width..self.width
        } else {
            0..edge_width
        };
        let edge_y = if bottom {
            self.height - edge_height..self.height
        } else {
            0..edge_height
        };
        for y in 0..self.height {
            for x in edge_x.clone() {
                self.pixels[y * self.width + x] = u8::from((hy0..hy1).contains(&y)) * 255;
            }
        }
        for x in 0..self.width {
            for y in edge_y.clone() {
                self.pixels[y * self.width + x] = u8::from((vx0..vx1).contains(&x)) * 255;
            }
        }
    }

    fn diagonal(&mut self, descending: bool, thickness: usize) {
        let max_x = self.width.saturating_sub(1) as f32;
        let max_y = self.height.saturating_sub(1) as f32;
        for y in 0..self.height {
            let target = if descending {
                y as f32 / max_y.max(1.) * max_x
            } else {
                (1. - y as f32 / max_y.max(1.)) * max_x
            };
            for x in 0..self.width {
                if (x as f32 - target).abs() <= thickness as f32 / 2. {
                    self.pixels[y * self.width + x] = 255;
                }
            }
        }
    }

    fn corner_triangle(&mut self, corner: Corner, outline: bool) {
        let w = self.width as isize;
        let h = self.height as isize;
        let points = match corner {
            Corner::TopLeft => ((0, 0), (w, 0), (0, h)),
            Corner::TopRight => ((0, 0), (w, 0), (w, h)),
            Corner::BottomLeft => ((0, 0), (0, h), (w, h)),
            Corner::BottomRight => ((0, h), (w, 0), (w, h)),
        };
        if outline {
            self.line(points.0, points.1, 1);
            self.line(points.1, points.2, 1);
            self.line(points.2, points.0, 1);
        } else {
            self.triangle(points.0, points.1, points.2, 255);
        }
    }

    fn triangle(&mut self, a: (isize, isize), b: (isize, isize), c: (isize, isize), alpha: u8) {
        let edge = |p: (isize, isize), q: (isize, isize), x: isize, y: isize| {
            (x - p.0) * (q.1 - p.1) - (y - p.1) * (q.0 - p.0)
        };
        for y in 0..self.height as isize {
            for x in 0..self.width as isize {
                let values = [edge(a, b, x, y), edge(b, c, x, y), edge(c, a, x, y)];
                if values.iter().all(|value| *value >= 0) || values.iter().all(|value| *value <= 0)
                {
                    self.pixels[y as usize * self.width + x as usize] = alpha;
                }
            }
        }
    }

    fn polygon(&mut self, points: &[(f32, f32)], alpha: u8) {
        if points.len() < 3 {
            return;
        }
        for y in 0..self.height {
            for x in 0..self.width {
                let sample_x = x as f32 + 0.5;
                let sample_y = y as f32 + 0.5;
                let mut inside = false;
                let mut previous = points.len() - 1;
                for current in 0..points.len() {
                    let (x0, y0) = points[previous];
                    let (x1, y1) = points[current];
                    let crosses = (y0 > sample_y) != (y1 > sample_y)
                        && sample_x < (x1 - x0) * (sample_y - y0) / (y1 - y0) + x0;
                    if crosses {
                        inside = !inside;
                    }
                    previous = current;
                }
                if inside {
                    self.pixels[y * self.width + x] = alpha;
                }
            }
        }
    }

    fn line(&mut self, a: (isize, isize), b: (isize, isize), thickness: usize) {
        let dx = (b.0 - a.0) as f32;
        let dy = (b.1 - a.1) as f32;
        let length2 = dx * dx + dy * dy;
        for y in 0..self.height {
            for x in 0..self.width {
                let t = if length2 == 0. {
                    0.
                } else {
                    (((x as isize - a.0) as f32 * dx + (y as isize - a.1) as f32 * dy) / length2)
                        .clamp(0., 1.)
                };
                let px = a.0 as f32 + t * dx;
                let py = a.1 as f32 + t * dy;
                if ((x as f32 - px).powi(2) + (y as f32 - py).powi(2)).sqrt()
                    <= thickness as f32 / 2.
                {
                    self.pixels[y * self.width + x] = 255;
                }
            }
        }
    }

    fn cubic_bezier(
        &mut self,
        start: (f32, f32),
        control_1: (f32, f32),
        control_2: (f32, f32),
        end: (f32, f32),
        thickness: usize,
    ) {
        let steps = (self.width.max(self.height) * 2).max(16);
        let pixel_point = |point: (f32, f32)| (point.0.round() as isize, point.1.round() as isize);
        let mut previous = start;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let one_minus_t = 1. - t;
            let point = (
                one_minus_t.powi(3) * start.0
                    + 3. * one_minus_t.powi(2) * t * control_1.0
                    + 3. * one_minus_t * t.powi(2) * control_2.0
                    + t.powi(3) * end.0,
                one_minus_t.powi(3) * start.1
                    + 3. * one_minus_t.powi(2) * t * control_1.1
                    + 3. * one_minus_t * t.powi(2) * control_2.1
                    + t.powi(3) * end.1,
            );
            self.line(pixel_point(previous), pixel_point(point), thickness);
            previous = point;
        }
    }

    fn extensible_hook(
        &mut self,
        part: usize,
        stem_x: isize,
        tip_x: isize,
        height: f32,
        thickness: usize,
    ) {
        match part {
            0 => {
                self.cubic_bezier(
                    (tip_x as f32, 0.),
                    (stem_x as f32, 0.),
                    (stem_x as f32, height * 0.2),
                    (stem_x as f32, height),
                    thickness,
                );
                self.vertical_edge_connector(stem_x, false, thickness);
            }
            1 => {
                self.line((stem_x, 0), (stem_x, height as isize), thickness);
                self.vertical_edge_connector(stem_x, true, thickness);
                self.vertical_edge_connector(stem_x, false, thickness);
            }
            _ => {
                self.cubic_bezier(
                    (stem_x as f32, 0.),
                    (stem_x as f32, height * 0.8),
                    (stem_x as f32, height),
                    (tip_x as f32, height),
                    thickness,
                );
                self.vertical_edge_connector(stem_x, true, thickness);
            }
        }
    }

    fn extensible_square_bracket(
        &mut self,
        part: usize,
        stem_x: isize,
        tip_x: isize,
        height: f32,
        thickness: usize,
    ) {
        self.line((stem_x, 0), (stem_x, height as isize), thickness);
        match part {
            0 => {
                self.line((stem_x, 0), (tip_x, 0), thickness);
                self.vertical_edge_connector(stem_x, false, thickness);
            }
            1 => {
                self.vertical_edge_connector(stem_x, true, thickness);
                self.vertical_edge_connector(stem_x, false, thickness);
            }
            _ => {
                self.line(
                    (stem_x, height as isize),
                    (tip_x, height as isize),
                    thickness,
                );
                self.vertical_edge_connector(stem_x, true, thickness);
            }
        }
    }

    fn extensible_curly_bracket(
        &mut self,
        part: usize,
        stem_x: isize,
        outer_x: isize,
        inner_x: isize,
        height: f32,
        thickness: usize,
    ) {
        match part {
            0 => {
                self.cubic_bezier(
                    (outer_x as f32, 0.),
                    (stem_x as f32, 0.),
                    (stem_x as f32, height * 0.2),
                    (stem_x as f32, height),
                    thickness,
                );
                self.vertical_edge_connector(stem_x, false, thickness);
            }
            1 => {
                self.cubic_bezier(
                    (stem_x as f32, 0.),
                    (stem_x as f32, height * 0.36),
                    (inner_x as f32, height * 0.34),
                    (inner_x as f32, height * 0.5),
                    thickness,
                );
                self.cubic_bezier(
                    (inner_x as f32, height * 0.5),
                    (inner_x as f32, height * 0.66),
                    (stem_x as f32, height * 0.64),
                    (stem_x as f32, height),
                    thickness,
                );
                self.vertical_edge_connector(stem_x, true, thickness);
                self.vertical_edge_connector(stem_x, false, thickness);
            }
            2 => {
                self.cubic_bezier(
                    (stem_x as f32, 0.),
                    (stem_x as f32, height * 0.8),
                    (stem_x as f32, height),
                    (outer_x as f32, height),
                    thickness,
                );
                self.vertical_edge_connector(stem_x, true, thickness);
            }
            _ => {
                self.line((stem_x, 0), (stem_x, height as isize), thickness);
                self.vertical_edge_connector(stem_x, true, thickness);
                self.vertical_edge_connector(stem_x, false, thickness);
            }
        }
    }

    fn extensible_integral(
        &mut self,
        top: bool,
        stem_x: isize,
        tip_x: isize,
        height: f32,
        thickness: usize,
    ) {
        self.extensible_hook(usize::from(!top) * 2, stem_x, tip_x, height, thickness);
    }

    fn extensible_curly_section(
        &mut self,
        descends_left: bool,
        left_x: isize,
        right_x: isize,
        height: f32,
        thickness: usize,
    ) {
        let (start_x, end_x) = if descends_left {
            (right_x, left_x)
        } else {
            (left_x, right_x)
        };
        self.cubic_bezier(
            (start_x as f32, 0.),
            (start_x as f32, height * 0.45),
            (end_x as f32, height * 0.55),
            (end_x as f32, height),
            thickness,
        );
        self.vertical_edge_connector(start_x, true, thickness);
        self.vertical_edge_connector(end_x, false, thickness);
    }

    fn extensible_summation(
        &mut self,
        top: bool,
        left_x: isize,
        right_x: isize,
        height: f32,
        thickness: usize,
    ) {
        if top {
            self.line((left_x, 0), (right_x, 0), thickness);
            self.line((left_x, 0), (right_x, height as isize), thickness);
            self.vertical_edge_connector(right_x, false, thickness);
        } else {
            self.line((right_x, 0), (left_x, height as isize), thickness);
            self.line(
                (left_x, height as isize),
                (right_x, height as isize),
                thickness,
            );
            self.vertical_edge_connector(right_x, true, thickness);
        }
    }

    fn horizontal_square_bracket(
        &mut self,
        cp: u32,
        left_x: isize,
        right_x: isize,
        height: f32,
        thickness: usize,
    ) {
        let top_y = if cp == 0x23b6 {
            (height * 0.36).round() as isize
        } else {
            0
        };
        let bottom_y = if cp == 0x23b6 {
            (height * 0.64).round() as isize
        } else {
            height as isize
        };
        if matches!(cp, 0x23b4 | 0x23b6) {
            self.line((left_x, top_y), (right_x, top_y), thickness);
            self.line(
                (left_x, top_y),
                (left_x, (top_y as f32 + height * 0.2).round() as isize),
                thickness,
            );
            self.line(
                (right_x, top_y),
                (right_x, (top_y as f32 + height * 0.2).round() as isize),
                thickness,
            );
        }
        if matches!(cp, 0x23b5 | 0x23b6) {
            self.line((left_x, bottom_y), (right_x, bottom_y), thickness);
            self.line(
                (left_x, (bottom_y as f32 - height * 0.2).round() as isize),
                (left_x, bottom_y),
                thickness,
            );
            self.line(
                (right_x, (bottom_y as f32 - height * 0.2).round() as isize),
                (right_x, bottom_y),
                thickness,
            );
        }
    }

    fn radical_bottom(&mut self, left_x: isize, right_x: isize, height: f32, thickness: usize) {
        let center_y = (height / 2.).round() as isize;
        let valley_x = ((left_x + right_x) as f32 * 0.42).round() as isize;
        let valley_y = (height * 0.78).round() as isize;
        self.line(
            (
                (left_x as f32 * 0.5).round() as isize,
                (height * 0.62).round() as isize,
            ),
            (left_x, (height * 0.58).round() as isize),
            thickness,
        );
        self.line(
            (left_x, (height * 0.58).round() as isize),
            (valley_x, valley_y),
            thickness,
        );
        self.line((valley_x, valley_y), (right_x, center_y), thickness);
        self.line(
            (right_x, center_y),
            (self.width as isize, center_y),
            thickness,
        );
        self.horizontal_edge_connector(center_y, false, thickness);
    }

    fn dentistry_symbol(&mut self, cp: u32, center_x: isize, height: f32, thickness: usize) {
        let (up, right, down, left) = match cp {
            0x23be => (false, true, true, false),
            0x23bf => (true, true, false, false),
            0x23c0 | 0x23c3 | 0x23c6 => (true, false, true, false),
            0x23c1 | 0x23c4 | 0x23c7 | 0x23c9 => (false, true, true, true),
            0x23c2 | 0x23c5 | 0x23c8 | 0x23ca => (true, true, false, true),
            0x23cb => (false, false, true, true),
            _ => (true, false, false, true),
        };
        self.junction(up, right, down, left, thickness, thickness);

        let center_y = height / 2.;
        let radius = (self.width.min(self.height) as f32 * 0.28).max(thickness as f32 * 1.5);
        match cp {
            0x23c0..=0x23c2 => self.circle(center_x as f32, center_y, radius, false, thickness),
            0x23c3..=0x23c5 => {
                let top = (center_x, (center_y - radius).round() as isize);
                let bottom_left = (
                    (center_x as f32 - radius).round() as isize,
                    (center_y + radius).round() as isize,
                );
                let bottom_right = (
                    (center_x as f32 + radius).round() as isize,
                    (center_y + radius).round() as isize,
                );
                self.line(top, bottom_left, thickness);
                self.line(bottom_left, bottom_right, thickness);
                self.line(bottom_right, top, thickness);
            }
            0x23c6..=0x23c8 => {
                let offset = match cp {
                    0x23c7 => radius * 0.55,
                    0x23c8 => -radius * 0.55,
                    _ => 0.,
                };
                self.wave(center_y + offset, radius * 0.3, thickness);
            }
            _ => {}
        }
    }

    fn horizontal_math_symbol(&mut self, cp: u32, height: f32, thickness: usize) {
        let width = self.width as f32;
        let left = width * 0.12;
        let right = width * 0.88;
        match cp {
            0x23dc => self.cubic_bezier(
                (left, height * 0.62),
                (left, height * 0.28),
                (right, height * 0.28),
                (right, height * 0.62),
                thickness,
            ),
            0x23dd => self.cubic_bezier(
                (left, height * 0.38),
                (left, height * 0.72),
                (right, height * 0.72),
                (right, height * 0.38),
                thickness,
            ),
            0x23de | 0x23df => {
                let top = cp == 0x23de;
                let base = if top { height * 0.62 } else { height * 0.38 };
                let crest = if top { height * 0.38 } else { height * 0.62 };
                self.cubic_bezier(
                    (left, base),
                    (width * 0.18, base),
                    (width * 0.18, crest),
                    (width * 0.34, crest),
                    thickness,
                );
                self.cubic_bezier(
                    (width * 0.34, crest),
                    (width * 0.44, crest),
                    (width * 0.44, base),
                    (width * 0.5, base),
                    thickness,
                );
                self.cubic_bezier(
                    (width * 0.5, base),
                    (width * 0.56, base),
                    (width * 0.56, crest),
                    (width * 0.66, crest),
                    thickness,
                );
                self.cubic_bezier(
                    (width * 0.66, crest),
                    (width * 0.82, crest),
                    (width * 0.82, base),
                    (right, base),
                    thickness,
                );
            }
            0x23e0 => {
                self.line(
                    (left.round() as isize, (height * 0.62).round() as isize),
                    (
                        (width * 0.26).round() as isize,
                        (height * 0.38).round() as isize,
                    ),
                    thickness,
                );
                self.line(
                    (
                        (width * 0.26).round() as isize,
                        (height * 0.38).round() as isize,
                    ),
                    (
                        (width * 0.74).round() as isize,
                        (height * 0.38).round() as isize,
                    ),
                    thickness,
                );
                self.line(
                    (
                        (width * 0.74).round() as isize,
                        (height * 0.38).round() as isize,
                    ),
                    (right.round() as isize, (height * 0.62).round() as isize),
                    thickness,
                );
            }
            _ => {
                self.line(
                    (left.round() as isize, (height * 0.38).round() as isize),
                    (
                        (width * 0.26).round() as isize,
                        (height * 0.62).round() as isize,
                    ),
                    thickness,
                );
                self.line(
                    (
                        (width * 0.26).round() as isize,
                        (height * 0.62).round() as isize,
                    ),
                    (
                        (width * 0.74).round() as isize,
                        (height * 0.62).round() as isize,
                    ),
                    thickness,
                );
                self.line(
                    (
                        (width * 0.74).round() as isize,
                        (height * 0.62).round() as isize,
                    ),
                    (right.round() as isize, (height * 0.38).round() as isize),
                    thickness,
                );
            }
        }
    }

    fn wave(&mut self, center_y: f32, amplitude: f32, thickness: usize) {
        let x0 = self.width as f32 * 0.2;
        let span = self.width as f32 * 0.6;
        let steps = (span.round() as usize * 2).max(16);
        let mut previous = (x0.round() as isize, center_y.round() as isize);
        for step in 1..=steps {
            let fraction = step as f32 / steps as f32;
            let point = (
                (x0 + fraction * span).round() as isize,
                (center_y + (fraction * std::f32::consts::TAU).sin() * amplitude).round() as isize,
            );
            self.line(previous, point, thickness);
            previous = point;
        }
    }

    fn vertical_edge_connector(&mut self, x: isize, top: bool, thickness: usize) {
        let x0 = x.saturating_sub((thickness / 2) as isize).max(0) as usize;
        let x1 = (x + thickness.div_ceil(2) as isize)
            .max(0)
            .min(self.width as isize) as usize;
        let edge_height = VECTOR_OVERSAMPLE.min(self.height);
        let (y0, y1) = if top {
            (0, edge_height)
        } else {
            (self.height.saturating_sub(edge_height), self.height)
        };
        self.fill(0, y0, self.width, y1, 0);
        self.fill(x0, y0, x1, y1, 255);
    }

    fn horizontal_edge_connector(&mut self, y: isize, left: bool, thickness: usize) {
        let y0 = y.saturating_sub((thickness / 2) as isize).max(0) as usize;
        let y1 = (y + thickness.div_ceil(2) as isize)
            .max(0)
            .min(self.height as isize) as usize;
        let edge_width = VECTOR_OVERSAMPLE.min(self.width);
        let (x0, x1) = if left {
            (0, edge_width)
        } else {
            (self.width.saturating_sub(edge_width), self.width)
        };
        self.fill(x0, 0, x1, self.height, 0);
        self.fill(x0, y0, x1, y1, 255);
    }

    fn chevron(&mut self, left: bool, thickness: usize) {
        let w = self.width as isize;
        let h = self.height as isize;
        if left {
            self.line((w, 0), (0, h / 2), thickness);
            self.line((0, h / 2), (w, h), thickness);
        } else {
            self.line((0, 0), (w, h / 2), thickness);
            self.line((w, h / 2), (0, h), thickness);
        }
    }

    fn powerline_round(&mut self, cp: u32, thickness: usize) {
        let left = matches!(cp, 0xe0b6 | 0xe0b7);
        let outline = matches!(cp, 0xe0b5 | 0xe0b7);
        let radius = self.width.min(self.height / 2).max(1) as f32;
        for y in 0..self.height {
            let edge = if y as f32 <= radius {
                (radius * radius - (y as f32 - radius).powi(2)).sqrt()
            } else if y as f32 >= self.height as f32 - radius {
                (radius * radius - (y as f32 - (self.height as f32 - radius)).powi(2)).sqrt()
            } else {
                radius
            };
            let boundary = if left { self.width as f32 - edge } else { edge };
            for x in 0..self.width {
                let inside = if left {
                    x as f32 >= boundary
                } else {
                    x as f32 <= boundary
                };
                let on_edge = (x as f32 - boundary).abs() <= thickness as f32;
                if (outline && on_edge) || (!outline && inside) {
                    self.pixels[y * self.width + x] = 255;
                }
            }
        }
    }

    fn powerline_hourglass(&mut self, flip: bool, thickness: usize) {
        let midpoint = self.height as f32 / 2.;
        for y in 0..self.height {
            let distance = (y as f32 - midpoint).abs();
            let boundary =
                self.width as f32 / 2. + distance / midpoint.max(1.) * self.width as f32 / 2.;
            for x in 0..self.width {
                let filled = if flip {
                    x as f32 >= self.width as f32 - boundary
                } else {
                    x as f32 <= boundary
                };
                let center_gap = (y as f32 - midpoint).abs() < thickness as f32 / 2.;
                if filled && !center_gap {
                    self.pixels[y * self.width + x] = 255;
                }
            }
        }
    }

    fn fading_line(&mut self, cp: u32, thickness: usize) {
        let horizontal = matches!(cp, 0xf5d2 | 0xf5d3);
        let reverse = matches!(cp, 0xf5d2 | 0xf5d4);
        if horizontal {
            let y0 = self.height / 2 - thickness / 2;
            for x in 0..self.width {
                let fraction = x as f32 / self.width.max(1) as f32;
                let alpha = if reverse { 1. - fraction } else { fraction };
                self.fill(x, y0, x + 1, y0 + thickness, (alpha * 255.).round() as u8);
            }
        } else {
            let x0 = self.width / 2 - thickness / 2;
            for y in 0..self.height {
                let fraction = y as f32 / self.height.max(1) as f32;
                let alpha = if reverse { 1. - fraction } else { fraction };
                self.fill(x0, y, x0 + thickness, y + 1, (alpha * 255.).round() as u8);
            }
        }
    }

    fn rounded_corner_for_branch(&mut self, corner: Corner, thickness: usize) {
        let cp = match corner {
            Corner::TopLeft => 0x256d,
            Corner::TopRight => 0x256e,
            Corner::BottomRight => 0x256f,
            Corner::BottomLeft => 0x2570,
        };
        self.rounded_corner(cp, thickness);
    }

    fn branch_arc_composite(&mut self, cp: u32, thickness: usize) {
        let (vertical, horizontal, corners): (bool, bool, &[Corner]) = match cp {
            0xf5da => (true, false, &[Corner::TopRight]),
            0xf5db => (true, false, &[Corner::BottomRight]),
            0xf5dc => (false, false, &[Corner::TopRight, Corner::BottomRight]),
            0xf5dd => (true, false, &[Corner::TopLeft]),
            0xf5de => (true, false, &[Corner::BottomLeft]),
            0xf5df => (false, false, &[Corner::TopLeft, Corner::BottomLeft]),
            0xf5e0 => (false, true, &[Corner::BottomLeft]),
            0xf5e1 => (false, true, &[Corner::BottomRight]),
            0xf5e2 => (false, false, &[Corner::BottomRight, Corner::BottomLeft]),
            0xf5e3 => (false, true, &[Corner::TopLeft]),
            0xf5e4 => (false, true, &[Corner::TopRight]),
            0xf5e5 => (false, false, &[Corner::TopRight, Corner::TopLeft]),
            0xf5e6 => (true, false, &[Corner::TopLeft, Corner::TopRight]),
            0xf5e7 => (true, false, &[Corner::BottomLeft, Corner::BottomRight]),
            0xf5e8 => (false, true, &[Corner::BottomLeft, Corner::TopLeft]),
            0xf5e9 => (false, true, &[Corner::TopRight, Corner::BottomRight]),
            0xf5ea => (true, false, &[Corner::TopLeft, Corner::BottomRight]),
            0xf5eb => (true, false, &[Corner::TopRight, Corner::BottomLeft]),
            0xf5ec => (false, true, &[Corner::TopLeft, Corner::BottomRight]),
            _ => (false, true, &[Corner::TopRight, Corner::BottomLeft]),
        };
        if vertical {
            self.junction(true, false, true, false, thickness, thickness);
        }
        if horizontal {
            self.junction(false, true, false, true, thickness, thickness);
        }
        for corner in corners {
            self.rounded_corner_for_branch(*corner, thickness);
        }
    }

    fn branch_node(&mut self, connections: u8, filled: bool, thickness: usize) {
        let cx = self.width / 2;
        let cy = self.height / 2;
        let radius = (self.width.min(self.height) / 3).max(thickness + 1);
        if connections & 0b0001 != 0 {
            self.fill(
                cx - thickness / 2,
                0,
                cx + thickness.div_ceil(2),
                cy.saturating_sub(radius),
                255,
            );
        }
        if connections & 0b0010 != 0 {
            self.fill(
                cx + radius,
                cy - thickness / 2,
                self.width,
                cy + thickness.div_ceil(2),
                255,
            );
        }
        if connections & 0b0100 != 0 {
            self.fill(
                cx - thickness / 2,
                cy + radius,
                cx + thickness.div_ceil(2),
                self.height,
                255,
            );
        }
        if connections & 0b1000 != 0 {
            self.fill(
                0,
                cy - thickness / 2,
                cx.saturating_sub(radius),
                cy + thickness.div_ceil(2),
                255,
            );
        }
        self.circle(cx as f32, cy as f32, radius as f32, filled, thickness);
    }

    fn circle(&mut self, cx: f32, cy: f32, radius: f32, filled: bool, thickness: usize) {
        for y in 0..self.height {
            for x in 0..self.width {
                let distance = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if (filled && distance <= radius)
                    || (!filled && (distance - radius).abs() <= thickness as f32 / 2.)
                {
                    self.pixels[y * self.width + x] = 255;
                }
            }
        }
    }

    fn edge_triangle(&mut self, edge: u32, alpha: u8) {
        let w = self.width as isize;
        let h = self.height as isize;
        let center = (w / 2, h / 2);
        let (a, b) = match edge {
            0 => ((0, 0), (0, h)),
            1 => ((0, 0), (w, 0)),
            2 => ((w, 0), (w, h)),
            _ => ((0, h), (w, h)),
        };
        self.triangle(center, a, b, alpha);
    }

    fn legacy_corner_diagonals(&mut self, index: usize) {
        const CORNERS: [u8; 15] = [
            0b0001, 0b0010, 0b0100, 0b1000, 0b0101, 0b1010, 0b1100, 0b0011, 0b1001, 0b0110, 0b1110,
            0b1101, 0b1011, 0b0111, 0b1111,
        ];
        let bits = CORNERS[index.min(CORNERS.len() - 1)];
        let center_x = self.width.div_ceil(2) as isize;
        let center_y = self.height.div_ceil(2) as isize;
        let thickness = ((self.width.min(self.height) as f32 * 0.11).round() as usize).max(1);
        // These names use *quadrants*, not cell corners. Match Ghostty and Unicode: each arm
        // joins the center of a vertical edge to the center of a horizontal edge.
        let arms = [
            ((center_x, 0), (0, center_y)),
            ((center_x, 0), (self.width as isize, center_y)),
            ((center_x, self.height as isize), (0, center_y)),
            (
                (center_x, self.height as isize),
                (self.width as isize, center_y),
            ),
        ];
        for (bit, (start, end)) in arms.into_iter().enumerate() {
            if bits & (1 << bit) != 0 {
                self.line(start, end, thickness);
            }
        }
    }

    fn legacy_cell_diagonal(&mut self, index: usize) {
        let positions = [
            (0, 0),
            (self.width as isize / 2, 0),
            (self.width as isize, 0),
            (0, self.height as isize / 2),
            (self.width as isize, self.height as isize / 2),
            (0, self.height as isize),
            (self.width as isize / 2, self.height as isize),
            (self.width as isize, self.height as isize),
        ];
        let a = positions[index % positions.len()];
        let b = positions[(index * 3 + 5) % positions.len()];
        self.line(a, b, 1);
    }

    fn legacy_circle_or_square(&mut self, index: usize) {
        let half_w = self.width / 4;
        let half_h = self.height / 4;
        let centers = [
            (self.width / 2, half_h),
            (self.width - half_w, self.height / 2),
            (self.width / 2, self.height - half_h),
            (half_w, self.height / 2),
        ];
        if index < 4 || (8..12).contains(&index) {
            let (cx, cy) = centers[index % 4];
            self.circle(
                cx as f32,
                cy as f32,
                half_w.min(half_h) as f32,
                index >= 8,
                1,
            );
        } else if index < 8 {
            let (cx, cy) = centers[(index - 4) % 4];
            self.fill(
                cx.saturating_sub(half_w),
                cy.saturating_sub(half_h),
                cx + half_w,
                cy + half_h,
                255,
            );
        } else {
            let corner = [
                Corner::TopRight,
                Corner::BottomLeft,
                Corner::BottomRight,
                Corner::TopLeft,
            ][index.saturating_sub(12).min(3)];
            self.corner_triangle(corner, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge_presence(mask: &[u8], width: usize, height: usize) -> [bool; 4] {
        [
            mask[..width].iter().any(|alpha| *alpha != 0),
            (0..height).any(|y| mask[y * width + width - 1] != 0),
            mask[(height - 1) * width..].iter().any(|alpha| *alpha != 0),
            (0..height).any(|y| mask[y * width] != 0),
        ]
    }

    fn column(mask: &[u8], width: usize, height: usize, x: usize) -> Vec<u8> {
        (0..height).map(|y| mask[y * width + x]).collect()
    }

    #[test]
    fn minimum_contrast_exemption_matches_terminal_graphics_ranges() {
        for character in [
            '─',
            '▒',
            '\u{e0b0}',
            '\u{e0d7}',
            '\u{1fbff}',
            '\u{1cc00}',
            '\u{1cebf}',
        ] {
            assert!(skips_minimum_contrast(character), "{character:?}");
        }
        for character in ['A', '⣿', '◢', '\u{e0d8}', '\u{1cbff}', '\u{1cec0}'] {
            assert!(!skips_minimum_contrast(character), "{character:?}");
        }
    }

    #[test]
    fn ghostty_sprite_ranges_are_all_intercepted() {
        let ranges = [
            0x2500..=0x257f,
            0x2580..=0x259f,
            0x2800..=0x28ff,
            0xf5d0..=0xf60d,
            0x1fb00..=0x1fbaf,
            0x1fbbd..=0x1fbbf,
            0x1fbce..=0x1fbef,
            0x1cc1b..=0x1cc1e,
            0x1cc21..=0x1cc2f,
            0x1cc30..=0x1cc3f,
            0x1cd00..=0x1cde5,
            0x1ce00..=0x1ce01,
            0x1ce0b..=0x1ce0c,
            0x1ce16..=0x1ce19,
            0x1ce51..=0x1ceaf,
        ];
        for range in ranges {
            for cp in range {
                let character = char::from_u32(cp).unwrap();
                assert!(
                    SpriteKind::for_char(character).is_some(),
                    "missing U+{cp:04X}"
                );
            }
        }
        for cp in [
            0x25e2, 0x25e3, 0x25e4, 0x25e5, 0x25f8, 0x25f9, 0x25fa, 0x25ff, 0xe0b0, 0xe0b1, 0xe0b2,
            0xe0b3, 0xe0b4, 0xe0b5, 0xe0b6, 0xe0b7, 0xe0b8, 0xe0b9, 0xe0ba, 0xe0bb, 0xe0bc, 0xe0bd,
            0xe0be, 0xe0bf, 0xe0d2, 0xe0d4,
        ] {
            assert!(SpriteKind::for_char(char::from_u32(cp).unwrap()).is_some());
        }
    }

    #[test]
    fn primary_terminal_drawing_ranges_have_builtin_cell_rasterizers() {
        for range in [
            0x2320..=0x2321,
            0x239b..=0x23cc,
            0x23dc..=0x23e1,
            0x2500..=0x257f,
            0x2580..=0x259f,
            0x2800..=0x28ff,
            0xf5d0..=0xf60d,
        ] {
            for cp in range {
                let character = char::from_u32(cp).unwrap();
                assert!(
                    rasterize(character, 16, 32, None).is_some(),
                    "missing builtin rasterizer for U+{cp:04X}"
                );
            }
        }
        for cp in [
            0x23d0, 0x25e2, 0x25e3, 0x25e4, 0x25e5, 0x25f8, 0x25f9, 0x25fa, 0x25ff, 0xe0b0, 0xe0b1,
            0xe0b2, 0xe0b3, 0xe0b4, 0xe0b5, 0xe0b6, 0xe0b7, 0xe0b8, 0xe0b9, 0xe0ba, 0xe0bb, 0xe0bc,
            0xe0bd, 0xe0be, 0xe0bf, 0xe0d2, 0xe0d4,
        ] {
            assert!(rasterize(char::from_u32(cp).unwrap(), 16, 32, None).is_some());
        }
    }

    #[test]
    fn typographic_fractions_are_left_to_core_text() {
        for character in "¼½¾⅐⅑⅒⅓⅔⅕⅖⅗⅘⅙⅚⅛⅜⅝⅞⅟↉".chars() {
            assert_eq!(SpriteKind::for_char(character), None);
            assert_eq!(rasterize(character, 17, 37, None), None);
        }
    }

    #[test]
    fn unicode_16_octants_fill_exact_cell_quarters() {
        for cp in 0x1cd00..=0x1cde5 {
            let character = char::from_u32(cp).unwrap();
            let mask = rasterize(character, 10, 20, None)
                .unwrap_or_else(|| panic!("missing octant sprite U+{cp:04X}"));
            assert_eq!(mask.len(), 10 * 20);
            assert!(mask.contains(&255));
        }

        // U+1CD00 BLOCK OCTANT-3 occupies the left half of the second quarter only.
        let third = rasterize('\u{1cd00}', 10, 20, None).unwrap();
        for y in 0..20 {
            for x in 0..10 {
                let expected = x < 5 && (5..10).contains(&y);
                assert_eq!(third[y * 10 + x] == 255, expected, "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn legacy_corner_diagonals_join_edge_centers_in_unicode_directions() {
        let width = 16;
        let height = 32;
        let expected_edges = [
            ('🮠', [true, false, false, true]),
            ('🮡', [true, true, false, false]),
            ('🮢', [false, false, true, true]),
            ('🮣', [false, true, true, false]),
            ('🮤', [true, false, true, true]),
            ('🮥', [true, true, true, false]),
            ('🮦', [false, true, true, true]),
            ('🮧', [true, true, false, true]),
        ];

        for (character, expected) in expected_edges {
            let mask = rasterize(character, width, height, None).unwrap();
            let actual = [
                mask[..width].iter().any(|alpha| *alpha != 0),
                (0..height).any(|y| mask[y * width + width - 1] != 0),
                mask[(height - 1) * width..].iter().any(|alpha| *alpha != 0),
                (0..height).any(|y| mask[y * width] != 0),
            ];
            assert_eq!(actual, expected, "wrong edge mapping for {character}");
            assert_eq!(mask[0], 0, "{character} must not start at a cell corner");
            assert_eq!(
                mask[width - 1],
                0,
                "{character} must not start at a cell corner"
            );
            assert!(
                mask.iter().any(|alpha| (1..=254).contains(alpha)),
                "{character} must retain antialiasing"
            );
        }
    }

    #[test]
    fn extensible_parentheses_connect_across_cell_rows() {
        let width = 16;
        let height = 32;
        for (upper, extension, lower) in [('⎛', '⎜', '⎝'), ('⎞', '⎟', '⎠')] {
            let upper = rasterize(upper, width, height, None).unwrap();
            let extension = rasterize(extension, width, height, None).unwrap();
            let lower = rasterize(lower, width, height, None).unwrap();

            assert_eq!(
                &upper[(height - 1) * width..],
                &extension[..width],
                "upper hook and extension must share the same boundary pixels"
            );
            assert_eq!(
                &extension[(height - 1) * width..],
                &lower[..width],
                "extension and lower hook must share the same boundary pixels"
            );
            assert!(
                upper.iter().any(|alpha| (1..=254).contains(alpha)),
                "curved hooks must retain antialiasing"
            );
        }
    }

    #[test]
    fn extensible_square_brackets_connect_across_cell_rows() {
        let width = 16;
        let height = 32;
        for (upper, extension, lower) in [('⎡', '⎢', '⎣'), ('⎤', '⎥', '⎦')] {
            let upper = rasterize(upper, width, height, None).unwrap();
            let extension = rasterize(extension, width, height, None).unwrap();
            let lower = rasterize(lower, width, height, None).unwrap();

            assert_eq!(&upper[(height - 1) * width..], &extension[..width]);
            assert_eq!(&extension[(height - 1) * width..], &lower[..width]);
        }
    }

    #[test]
    fn extensible_curly_brackets_connect_through_shared_extensions() {
        let width = 16;
        let height = 32;
        let extension = rasterize('⎪', width, height, None).unwrap();
        for (upper, middle, lower) in [('⎧', '⎨', '⎩'), ('⎫', '⎬', '⎭')] {
            let upper = rasterize(upper, width, height, None).unwrap();
            let middle = rasterize(middle, width, height, None).unwrap();
            let lower = rasterize(lower, width, height, None).unwrap();

            assert_eq!(&upper[(height - 1) * width..], &extension[..width]);
            assert_eq!(&extension[(height - 1) * width..], &middle[..width]);
            assert_eq!(&middle[(height - 1) * width..], &extension[..width]);
            assert_eq!(&extension[(height - 1) * width..], &lower[..width]);
            assert!(
                upper.iter().any(|alpha| (1..=254).contains(alpha)),
                "curved hooks must retain antialiasing"
            );
        }

        let left_middle = rasterize('⎨', width, height, None).unwrap();
        let right_middle = rasterize('⎬', width, height, None).unwrap();
        assert!((0..height).any(|y| left_middle[y * width + width / 4] != 0));
        assert!((0..height).any(|y| right_middle[y * width + width * 3 / 4] != 0));
    }

    #[test]
    fn integral_and_vertical_extensions_connect_across_every_row_boundary() {
        let width = 16;
        let height = 32;
        let upper = rasterize('⌠', width, height, None).unwrap();
        let extension = rasterize('⎮', width, height, None).unwrap();
        let lower = rasterize('⌡', width, height, None).unwrap();
        let plain_extension = rasterize('⏐', width, height, None).unwrap();

        assert_eq!(&upper[(height - 1) * width..], &extension[..width]);
        assert_eq!(&extension[(height - 1) * width..], &lower[..width]);
        assert_eq!(
            &plain_extension[..width],
            &plain_extension[(height - 1) * width..]
        );
    }

    #[test]
    fn remaining_vertical_math_sections_share_exact_boundary_pixels() {
        let width = 16;
        let height = 32;
        let descends_left = rasterize('⎰', width, height, None).unwrap();
        let descends_right = rasterize('⎱', width, height, None).unwrap();
        let summation_top = rasterize('⎲', width, height, None).unwrap();
        let summation_bottom = rasterize('⎳', width, height, None).unwrap();

        assert_eq!(
            &descends_left[(height - 1) * width..],
            &descends_right[..width]
        );
        assert_eq!(
            &descends_right[(height - 1) * width..],
            &descends_left[..width]
        );
        assert_eq!(
            &summation_top[(height - 1) * width..],
            &summation_bottom[..width]
        );
    }

    #[test]
    fn horizontal_extensions_and_scan_lines_reach_both_cell_edges() {
        let width = 16;
        let height = 32;
        for character in ['⎯', '⎺', '⎻', '⎼', '⎽'] {
            let mask = rasterize(character, width, height, None).unwrap();
            assert_eq!(
                column(&mask, width, height, 0),
                column(&mask, width, height, width - 1),
                "{character} must tile horizontally without a seam"
            );
        }

        let radical = rasterize('⎷', width, height, None).unwrap();
        let extension = rasterize('⎯', width, height, None).unwrap();
        assert_eq!(
            column(&radical, width, height, width - 1),
            column(&extension, width, height, 0),
            "the radical roof must join the horizontal extension"
        );
    }

    #[test]
    fn dentistry_symbols_expose_only_their_named_connection_edges() {
        let width = 16;
        let height = 32;
        let expected = [
            ('⎾', [false, true, true, false]),
            ('⎿', [true, true, false, false]),
            ('⏀', [true, false, true, false]),
            ('⏁', [false, true, true, true]),
            ('⏂', [true, true, false, true]),
            ('⏃', [true, false, true, false]),
            ('⏄', [false, true, true, true]),
            ('⏅', [true, true, false, true]),
            ('⏆', [true, false, true, false]),
            ('⏇', [false, true, true, true]),
            ('⏈', [true, true, false, true]),
            ('⏉', [false, true, true, true]),
            ('⏊', [true, true, false, true]),
            ('⏋', [false, false, true, true]),
            ('⏌', [true, false, false, true]),
        ];

        for (character, edges) in expected {
            let mask = rasterize(character, width, height, None).unwrap();
            assert_eq!(edge_presence(&mask, width, height), edges, "{character}");
        }
    }

    #[test]
    fn horizontal_math_brackets_are_real_antialiased_sprites() {
        for character in "⎴⎵⎶⏜⏝⏞⏟⏠⏡".chars() {
            let mask = rasterize(character, 16, 32, None).unwrap();
            assert!(mask.iter().any(|alpha| *alpha != 0), "{character}");
            assert!(
                mask.iter().any(|alpha| (1..=254).contains(alpha)),
                "{character} must retain antialiasing"
            );
        }
    }

    #[test]
    fn block_shades_are_uniform_cell_alpha() {
        for (character, alpha) in [('░', 0x40), ('▒', 0x80), ('▓', 0xc0)] {
            let mask = rasterize(character, 13, 27, None).unwrap();
            assert!(mask.iter().all(|value| *value == alpha));
        }
    }

    #[test]
    fn neighboring_full_blocks_reach_every_cell_edge() {
        let mask = rasterize('█', 13, 27, None).unwrap();
        assert!(mask.iter().all(|value| *value == 255));
    }

    #[test]
    fn segmented_digits_never_fall_through_to_last_resort() {
        for cp in 0x1fbf0..=0x1fbf9 {
            let character = char::from_u32(cp).unwrap();
            assert_eq!(
                SpriteKind::for_char(character),
                Some(SpriteKind::SegmentedDigit)
            );
            let mask = rasterize(character, 16, 32, None).unwrap();
            assert_eq!(mask.len(), 16 * 32);
            assert!(mask.iter().any(|alpha| *alpha != 0), "U+{cp:04X}");
        }
    }

    #[test]
    fn notcurses_legacy_computing_ranges_have_real_rasterizers() {
        for range in [
            0x1fb00..=0x1fbaf,
            0x1fbbd..=0x1fbbf,
            0x1fbce..=0x1fbef,
            0x1fbf0..=0x1fbf9,
        ] {
            for cp in range {
                let character = char::from_u32(cp).unwrap();
                let mask = rasterize(character, 16, 32, None)
                    .unwrap_or_else(|| panic!("missing rasterizer for U+{cp:04X}"));
                assert_eq!(mask.len(), 16 * 32, "U+{cp:04X}");
            }
        }
    }

    #[test]
    fn smooth_mosaics_fill_their_selected_cell_edges() {
        for cp in 0x1fb3c..=0x1fb67 {
            let mask = rasterize(char::from_u32(cp).unwrap(), 16, 32, None).unwrap();
            assert!(mask.iter().any(|alpha| *alpha != 0), "U+{cp:04X}");
            assert!(
                (0..32).any(|y| mask[y * 16] != 0)
                    || (0..32).any(|y| mask[y * 16 + 15] != 0)
                    || mask[..16].iter().any(|alpha| *alpha != 0)
                    || mask[31 * 16..].iter().any(|alpha| *alpha != 0),
                "U+{cp:04X} must reach at least one cell edge"
            );
        }
    }

    #[test]
    fn rounded_box_corners_join_adjacent_straight_lines_at_cell_edges() {
        let width = 16;
        let height = 32;
        let horizontal = rasterize('─', width, height, None).unwrap();
        let vertical = rasterize('│', width, height, None).unwrap();

        for character in ['╭', '╰'] {
            let corner = rasterize(character, width, height, None).unwrap();
            for y in 0..height {
                assert_eq!(
                    corner[y * width + width - 1],
                    horizontal[y * width],
                    "{character} must join a following horizontal line without a seam"
                );
            }
        }

        let top = rasterize('╰', width, height, None).unwrap();
        let bottom = rasterize('╭', width, height, None).unwrap();
        for x in 0..width {
            assert_eq!(
                top[x],
                vertical[(height - 1) * width + x],
                "╰ must join a vertical line above"
            );
            assert_eq!(
                bottom[(height - 1) * width + x],
                vertical[x],
                "╭ must join a vertical line below"
            );
        }
    }

    #[test]
    fn rounded_box_corners_have_antialiased_curve_edges() {
        for character in ['╭', '╮', '╯', '╰'] {
            let mask = rasterize(character, 16, 32, None).unwrap();
            assert!(
                mask.iter().any(|alpha| (1..=254).contains(alpha)),
                "{character} must contain partial coverage at its curved edge"
            );
        }
    }

    #[test]
    fn mixed_box_junctions_keep_their_directional_stroke_weights() {
        use LineStyle::{Double as D, Heavy as H, Light as L, None as N};

        assert_eq!(box_lines('┝' as u32), Some(BoxLines::new(L, H, L, N)));
        assert_eq!(box_lines('┖' as u32), Some(BoxLines::new(H, L, N, N)));
        assert_eq!(box_lines('╒' as u32), Some(BoxLines::new(N, D, L, N)));
        assert_eq!(box_lines('╤' as u32), Some(BoxLines::new(N, D, L, D)));
    }
}
