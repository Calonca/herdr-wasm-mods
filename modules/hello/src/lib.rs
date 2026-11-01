//! The smallest module: a counter, two buttons and a canvas. A template for new modules.

use wasm_mods_sdk::*;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Hello {
    clicks: u32,
    dots: Vec<(f32, f32)>,
}

impl Frame for Hello {
    fn update(&mut self, input: &Input) -> Output {
        for event in &input.events {
            match event {
                Event::Clicked { id } if id == "more" => self.clicks += 1,
                Event::Clicked { id } if id == "clear" => self.dots.clear(),
                // The same from the keyboard, a gamepad or the hints under the pane.
                Event::Action(Action::Primary) => self.clicks += 1,
                Event::Action(Action::Secondary) => self.dots.clear(),
                Event::Pointer { id, x, y, kind: PointerKind::Down | PointerKind::Drag } if id == "pad" => {
                    self.dots.push((*x, *y));
                }
                _ => {}
            }
        }
        let t = &input.theme;
        let mut shapes = vec![Shape::Rect {
            x: 0.0,
            y: 0.0,
            w: 320.0,
            h: 160.0,
            // The theme's tile corners (density).
            radius: t.density.tile_radius,
            fill: with_alpha(t.surface, 0x80),
            stroke: Some((1.0, t.border)),
        }];
        shapes.extend(self.dots.iter().map(|&(x, y)| Shape::Circle { x, y, r: 3.0, fill: t.accent }));
        Output::new(
            "hello",
            column([
                heading("hello from WebAssembly"),
                muted(format!("{} clicks · {} dots", self.clicks, self.dots.len())),
                row([button("more", "click me"), button("clear", "clear")]),
                muted("draw on it"),
                canvas("pad", 320.0, 160.0, shapes),
            ]),
        )
        .action(Action::Primary, "count")
        .action(Action::Secondary, "clear")
    }
}

export_frame!(Hello);
manifest!(description: "a counter, two buttons and a canvas: the template for new modules", capabilities: []);
