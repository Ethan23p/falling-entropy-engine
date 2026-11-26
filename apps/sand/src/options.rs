//! The options panel (the doc's View): the brush's controls, stacked vertically with their names
//! written across, on the right of the lattice; in portrait, in rows below it. Drawn from the
//! engine's own primitives.

use engine::glam::Vec2;
use engine::{Screen, Shapes, View, rgb, rgba};

use crate::view::{CONFIG_BUTTON, Rect};

/// The brush's tools, in order: what each paints.
pub const TOOLS: [(&str, sim::Material, u32); 5] = [
    ("SAND", sim::Material::Sand, 0xdeb76e),
    ("WATER", sim::Material::Water, 0x2a69c5),
    ("AIR", sim::Material::Air, 0x9aa6c8),
    ("SLATE", sim::Material::Slate, 0x525c6b),
    ("ERASE", sim::Material::Vacuum, 0x5a2a86),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Tool(usize),
    Smaller,
    /// Shows the size; not a button.
    Size,
    Larger,
    Clear,
}

/// Each control and where it sits, within `area`.
pub fn controls(area: Rect, portrait: bool, s: Screen) -> Vec<(Control, Rect)> {
    let gap = s.em(0.5);
    let mut out = Vec::new();
    if !portrait {
        // One column: the tools, then size, then clear.
        let h = s.em(2.75);
        let w = area.size.x;
        let mut y = area.min.y;
        let row = |out: &mut Vec<_>, y: &mut f32, items: &[(Control, f32)]| {
            let mut x = area.min.x;
            let total: f32 = items.iter().map(|i| i.1).sum();
            for &(c, share) in items {
                let iw = (w - gap * (items.len() - 1) as f32) * share / total;
                out.push((c, Rect::new(Vec2::new(x, *y), Vec2::new(iw, h))));
                x += iw + gap;
            }
            *y += h + gap;
        };
        for i in 0..TOOLS.len() {
            row(&mut out, &mut y, &[(Control::Tool(i), 1.0)]);
        }
        y += s.em(0.75);
        row(&mut out, &mut y, &[(Control::Smaller, 1.0), (Control::Size, 2.0), (Control::Larger, 1.0)]);
        y += gap;
        row(&mut out, &mut y, &[(Control::Clear, 1.0)]);
    } else {
        // Two rows: the tools; then size and clear, clear of the config button's corner.
        let h = (area.size.y - gap) * 0.5;
        let n = TOOLS.len() as f32;
        let w = (area.size.x - gap * (n - 1.0)) / n;
        for i in 0..TOOLS.len() {
            let min = Vec2::new(area.min.x + i as f32 * (w + gap), area.min.y);
            out.push((Control::Tool(i), Rect::new(min, Vec2::new(w, h))));
        }
        let y = area.min.y + h + gap;
        let room = area.size.x - s.em(CONFIG_BUTTON);
        let items = [(Control::Smaller, 1.0), (Control::Size, 1.6), (Control::Larger, 1.0), (Control::Clear, 1.6)];
        let total: f32 = items.iter().map(|i| i.1).sum();
        let mut x = area.min.x;
        for (c, share) in items {
            let iw = (room - 3.0 * gap) * share / total;
            out.push((c, Rect::new(Vec2::new(x, y), Vec2::new(iw, h))));
            x += iw + gap;
        }
    }
    out
}

pub fn at(area: Rect, portrait: bool, s: Screen, p: Vec2) -> Option<Control> {
    controls(area, portrait, s).into_iter().find(|(c, r)| *c != Control::Size && r.contains(p)).map(|(c, _)| c)
}

const INK: u32 = 0xeeeef2;
const SURFACE: u32 = 0x1b1f2a;

/// Draws the panel. `tool` is the chosen tool, `radius` the brush's size.
pub fn draw(shapes: &mut Shapes, area: Rect, portrait: bool, s: Screen, tool: usize, radius: i32) {
    shapes.view = View::PIXELS;
    let text = s.em(1.0);
    for (c, r) in controls(area, portrait, s) {
        let chosen = c == Control::Tool(tool);
        let corner = s.em(0.6).min(r.size.y * 0.5);
        if c != Control::Size {
            let fill = if chosen { rgb(0x2c3346) } else { rgb(SURFACE) };
            let outline = if chosen { rgba(INK, 0.85) } else { rgba(INK, 0.18) };
            shapes.rounded_rect(r.center(), r.size, corner, fill).outline(s.em(0.08), outline);
        }
        match c {
            Control::Tool(i) => {
                let (name, _, color) = TOOLS[i];
                let swatch = s.em(0.45);
                let label_w = shapes.text_width(text, name);
                if !portrait && r.size.x > label_w + swatch * 5.0 {
                    // The swatch, then the name, left to right.
                    let left = r.min.x + s.em(0.9);
                    shapes.circle(Vec2::new(left + swatch, r.center().y), swatch, rgb(color));
                    let x = left + swatch * 2.0 + s.em(0.7) + label_w * 0.5;
                    shapes.text(Vec2::new(x, r.center().y), text, name, rgb(INK));
                } else {
                    // Narrow (portrait): the swatch above the name.
                    let c = r.center();
                    shapes.circle(c - Vec2::new(0.0, r.size.y * 0.2), swatch, rgb(color));
                    shapes.text(c + Vec2::new(0.0, r.size.y * 0.2), text * 0.85, name, rgb(INK));
                }
            }
            Control::Smaller => {
                shapes.text(r.center(), text * 1.3, "-", rgb(INK));
            }
            Control::Larger => {
                shapes.text(r.center(), text * 1.3, "+", rgb(INK));
            }
            Control::Clear => {
                shapes.text(r.center(), text, "CLEAR", rgb(INK));
            }
            Control::Size => {
                // The radius of each hex, and the cells a stamp covers.
                let cells = 4 * (3 * radius * radius + 3 * radius + 1);
                let label = format!("SIZE {radius}");
                let c = r.center();
                shapes.text(c - Vec2::new(0.0, text * 0.45), text * 0.9, &label, rgb(INK));
                shapes.text(c + Vec2::new(0.0, text * 0.6), text * 0.7, &format!("{cells} cells"), rgba(INK, 0.6));
            }
        }
    }
}
