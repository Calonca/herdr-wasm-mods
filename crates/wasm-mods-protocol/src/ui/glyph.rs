//! Small symbols as canvas shapes (hexagon, triangle, square, circle, star, plus, cross,
//! diamond), to mark things apart by shape as well as colour.

use crate::*;

/// The outline of identity symbol `kind` (0–7, wrapping: hexagon, triangle, square,
/// circle, star, plus, cross, diamond) of radius `r` centred on `(x, y)`; none for the
/// circle. Every outline is star-shaped around its centre.
pub fn glyph_points(x: f32, y: f32, r: f32, kind: usize) -> Option<Vec<(f32, f32)>> {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, TAU};
    let polygon = |n: usize, start: f32, radius: &dyn Fn(usize) -> f32| -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| {
                let a = start + i as f32 / n as f32 * TAU;
                (x + a.cos() * radius(i), y + a.sin() * radius(i))
            })
            .collect()
    };
    // A plus with thick arms, turned by `turn`: closed, so it can be filled.
    let cross = |turn: f32| -> Vec<(f32, f32)> {
        let (a, l) = (r * 0.42, r * 1.05);
        let arm = [(a, -l), (a, -a), (l, -a), (l, a), (a, a), (a, l), (-a, l), (-a, a), (-l, a), (-l, -a), (-a, -a), (-a, -l)];
        let (s, c) = turn.sin_cos();
        arm.iter().map(|&(px, py)| (x + px * c - py * s, y + px * s + py * c)).collect()
    };
    Some(match kind % 8 {
        0 => polygon(6, -FRAC_PI_2, &|_| r),
        1 => polygon(3, -FRAC_PI_2, &|_| r * 1.1),
        2 => polygon(4, TAU / 8.0, &|_| r * 1.1),
        3 => return None,
        4 => polygon(10, -FRAC_PI_2, &|i| if i % 2 == 0 { r * 1.15 } else { r * 0.55 }),
        5 => cross(0.0),
        6 => cross(FRAC_PI_4),
        _ => polygon(4, -FRAC_PI_2, &|_| r * 1.1),
    })
}

/// Identity symbol `kind` (see [`glyph_points`]) as canvas shapes centred on `(x, y)`:
/// outlined in `color`, filled with `fill` when given (the host fills it by the agent's
/// state: `ansi[1]` blocked, `ansi[3]` working, `ansi[2]` done).
pub fn glyph_shapes(x: f32, y: f32, r: f32, kind: usize, color: Color, fill: Option<Color>) -> Vec<Shape> {
    // A thinner outline around a fill, so the fill still shows on small symbols.
    let stroke = Some((if fill.is_some() { 1.2 } else { 1.6 }, color));
    let points = glyph_points(x, y, r, kind).unwrap_or_else(|| {
        (0..24)
            .map(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                (x + a.cos() * r * 0.9, y + a.sin() * r * 0.9)
            })
            .collect()
    });
    let mut out = Vec::new();
    if let Some(fill) = fill {
        // The canvas fills convex shapes: the star and crosses as a fan from the centre.
        if matches!(kind % 8, 4..=6) {
            let n = points.len();
            for i in 0..n {
                out.push(Shape::Path { points: vec![(x, y), points[i], points[(i + 1) % n]], closed: true, fill: Some(fill), stroke: None });
            }
        } else {
            out.push(Shape::Path { points: points.clone(), closed: true, fill: Some(fill), stroke: None });
        }
    }
    out.push(Shape::Path { points, closed: true, fill: None, stroke });
    out
}
