//! Big canvases drawn by the host. herdr-app runs mods on an interpreter (QuickJS):
//! thousands of shapes cost it a second or more to read and draw, past what a redraw
//! may take. So a canvas with many shapes has its rectangles, circles, lines and paths
//! written by the host into one SVG picture (in the images folder, named by its
//! content's hash), which the mod shows as a single [`Shape::Image`] under the rest:
//! texts, icons and pictures stay shapes, drawn over it.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use sha2::{Digest, Sha256};
use wasm_mods_protocol::{Color, Coords, Fit, ImageSrc, ListItem, Node, Shape};

use crate::images::Shown;

/// Canvases with more shapes than this are drawn here.
pub const MANY: usize = 150;
/// The picture's longest side, in pixels.
const SIDE_MAX: f32 = 4096.0;

/// Draws the big canvases of `ui` into SVG files in `dir`. `zooms`: each canvas's zoom
/// (from its last view), to size the picture so it's sharp. Returns the pictures by
/// their keys (`canvas:<id>`), to send with what the module shows.
pub fn flatten(ui: &mut Node, dir: &Path, zooms: &HashMap<String, f32>) -> BTreeMap<String, Shown> {
    let mut out = BTreeMap::new();
    walk(ui, &mut |id, shapes| {
        let zoom = zooms.get(id).copied().unwrap_or(1.0).clamp(0.01, 64.0);
        if let Some((key, shown, image)) = draw(id, shapes, dir, zoom) {
            shapes.retain(|s| matches!(s, Shape::Text { .. } | Shape::Image { .. }));
            shapes.insert(0, image);
            out.insert(key, shown);
        }
    });
    out
}

fn walk(n: &mut Node, f: &mut impl FnMut(&str, &mut Vec<Shape>)) {
    match n {
        Node::Column { children, .. } | Node::Row { children, .. } => children.iter_mut().for_each(|c| walk(c, f)),
        Node::List { items, .. } => items.iter_mut().filter_map(|i: &mut ListItem| i.child.as_deref_mut()).for_each(|c| walk(c, f)),
        Node::Popup { child, .. } => walk(child, f),
        Node::Canvas { id, shapes, coords: Coords::Points, .. } if shapes.len() > MANY => f(id, shapes),
        _ => {}
    }
}

fn flat(s: &Shape) -> bool {
    matches!(s, Shape::Rect { .. } | Shape::Circle { .. } | Shape::Line { .. } | Shape::Path { .. })
}

/// `#rrggbb` and the opacity of an RGBA colour.
fn paint(c: Color) -> (String, f32) {
    (format!("#{:06x}", c >> 8), (c & 0xff) as f32 / 255.0)
}

/// The SVG of `shapes`' flat ones: the picture's key, file and size, and the image shape
/// that shows it in their bounds.
fn draw(id: &str, shapes: &[Shape], dir: &Path, zoom: f32) -> Option<(String, Shown, Shape)> {
    // Their bounds, in the canvas's coordinates.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut grow = |x: f32, y: f32, pad: f32| {
        x0 = x0.min(x - pad);
        y0 = y0.min(y - pad);
        x1 = x1.max(x + pad);
        y1 = y1.max(y + pad);
    };
    for s in shapes.iter().filter(|s| flat(s)) {
        match s {
            Shape::Rect { x, y, w, h, stroke, .. } => {
                let pad = stroke.map(|s| s.0).unwrap_or(0.0);
                grow(*x, *y, pad);
                grow(x + w, y + h, pad);
            }
            Shape::Circle { x, y, r, .. } => grow(*x, *y, *r),
            Shape::Line { x1, y1, x2, y2, width, .. } => {
                grow(*x1, *y1, *width);
                grow(*x2, *y2, *width);
            }
            Shape::Path { points, stroke, .. } => {
                let pad = stroke.map(|s| s.0).unwrap_or(0.0);
                points.iter().for_each(|(x, y)| grow(*x, *y, pad));
            }
            _ => {}
        }
    }
    if !(x0.is_finite() && x1 > x0 && y1 > y0) {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    // Pixels: twice the zoom (sharp on scaled screens), at most SIDE_MAX a side.
    let scale = (zoom * 2.0).min(SIDE_MAX / w.max(h));
    let (pw, ph) = ((w * scale).ceil().max(1.0) as u32, (h * scale).ceil().max(1.0) as u32);
    let mut svg = String::with_capacity(shapes.len() * 80);
    let _ = write!(svg, r#"<svg xmlns="http://www.w3.org/2000/svg" width="{pw}" height="{ph}" viewBox="{x0} {y0} {w} {h}">"#);
    for s in shapes.iter().filter(|s| flat(s)) {
        match s {
            Shape::Rect { x, y, w, h, radius, fill, stroke } => {
                let (f, fo) = paint(*fill);
                let _ = write!(svg, r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{f}" fill-opacity="{fo}""#);
                if let Some((sw, sc)) = stroke {
                    let (c, o) = paint(*sc);
                    let _ = write!(svg, r#" stroke="{c}" stroke-opacity="{o}" stroke-width="{sw}""#);
                }
                svg.push_str("/>");
            }
            Shape::Circle { x, y, r, fill } => {
                let (f, fo) = paint(*fill);
                let _ = write!(svg, r#"<circle cx="{x}" cy="{y}" r="{r}" fill="{f}" fill-opacity="{fo}"/>"#);
            }
            Shape::Line { x1, y1, x2, y2, width, color } => {
                let (c, o) = paint(*color);
                let _ = write!(
                    svg,
                    r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{c}" stroke-opacity="{o}" stroke-width="{width}" stroke-linecap="round"/>"#
                );
            }
            Shape::Path { points, closed, fill, stroke } => {
                let tag = if *closed { "polygon" } else { "polyline" };
                let _ = write!(svg, r#"<{tag} points=""#);
                for (x, y) in points {
                    let _ = write!(svg, "{x},{y} ");
                }
                svg.push('"');
                match fill {
                    Some(f) => {
                        let (f, fo) = paint(*f);
                        let _ = write!(svg, r#" fill="{f}" fill-opacity="{fo}""#);
                    }
                    None => svg.push_str(r#" fill="none""#),
                }
                if let Some((sw, sc)) = stroke {
                    let (c, o) = paint(*sc);
                    let _ = write!(svg, r#" stroke="{c}" stroke-opacity="{o}" stroke-width="{sw}" stroke-linejoin="round" stroke-linecap="round""#);
                }
                svg.push_str("/>");
            }
            _ => {}
        }
    }
    svg.push_str("</svg>");
    let hash = Sha256::digest(svg.as_bytes());
    let name: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect::<String>() + ".svg";
    let path = dir.join(&name);
    if !path.exists() {
        let tmp = dir.join(format!(".{name}.{}", std::process::id()));
        std::fs::write(&tmp, &svg).and_then(|_| std::fs::rename(&tmp, &path)).ok()?;
    }
    let key = format!("canvas:{id}");
    let image = Shape::Image {
        x: x0,
        y: y0,
        w,
        h,
        src: ImageSrc::Cached(key.clone()),
        fit: Fit::Fill,
        rounding: 0.0,
        opacity: 1.0,
    };
    Some((key, Shown { file: name, width: pw, height: ph }, image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_canvases_become_one_picture_under_their_texts() {
        let dir = std::env::temp_dir().join(format!("wasm-mods-canvas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut shapes: Vec<Shape> = (0..200).map(|i| Shape::Circle { x: i as f32 * 10.0, y: 5.0, r: 2.0, fill: 0xff0000ff }).collect();
        shapes.push(Shape::Text { x: 0.0, y: 0.0, text: "label".into(), size: 12.0, color: 0xffffffff });
        let mut ui = Node::Column {
            children: vec![
                wasm_mods_protocol::ui::canvas("world", 100.0, 100.0, shapes),
                wasm_mods_protocol::ui::canvas("small", 10.0, 10.0, vec![Shape::Circle { x: 1.0, y: 1.0, r: 1.0, fill: 0 }]),
            ],
            gap: 0.0,
            layout: Default::default(),
            style: Default::default(),
            id: None,
            tooltip: None,
        };
        let shown = flatten(&mut ui, &dir, &HashMap::new());
        let Node::Column { children, .. } = &ui else { panic!() };
        let Node::Canvas { shapes, .. } = &children[0] else { panic!() };
        assert!(matches!(&shapes[..], [Shape::Image { x, w, .. }, Shape::Text { .. }] if *x == -2.0 && *w == 1994.0));
        let Node::Canvas { shapes, .. } = &children[1] else { panic!() };
        assert_eq!(shapes.len(), 1, "a small canvas stays as it is");
        let svg = std::fs::read_to_string(dir.join(&shown["canvas:world"].file)).unwrap();
        assert_eq!(svg.matches("<circle").count(), 200);
        assert!(svg.contains(r##"fill="#ff0000" fill-opacity="1""##));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
