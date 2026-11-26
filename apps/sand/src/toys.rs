//! The toy-case display (the doc's View): every toy case running in its own lattice, at its own
//! size, in a strip beside the main lattice (left in landscape, on top in portrait). Each starts
//! paused; a tap plays or pauses it. Small lattices draw with cells the size of the main
//! lattice's; larger ones shrink to fit. Dragging scrolls the strip.

use engine::glam::Vec2;
use engine::{Screen, Shapes, View, rgb, rgba};
use scenes::Scene;

use crate::gpu::Lattice;
use crate::view::{Fit, Rect};

pub struct Toy {
    pub scene: &'static Scene,
    pub lattice: Option<Lattice>,
    pub playing: bool,
    /// Phases run since it was built, for its script.
    pub phase: u32,
}

/// Every scene but the blank one.
pub fn all() -> Vec<Toy> {
    scenes::ALL
        .iter()
        .filter(|s| s.name != "empty")
        .map(|scene| Toy { scene, lattice: None, playing: false, phase: 0 })
        .collect()
}

/// The label's height under each toy, in ems.
const LABEL: f32 = 1.6;

/// Where each toy's lattice draws (its fit) and its whole slot (lattice and label), in the strip
/// `area`, scrolled by `scroll` pixels along it. `scale` is the main lattice's (see `Fit::scale`).
pub fn arrange(
    toys: &[Toy],
    area: Rect,
    portrait: bool,
    s: Screen,
    scale: f32,
    hex_px: f32,
    scroll: f32,
) -> Vec<(Fit, Rect)> {
    let label = s.em(LABEL);
    let gap = s.em(0.75);
    let mut along = -scroll;
    toys.iter()
        .map(|t| {
            let (w, h) = t.scene.size;
            if !portrait {
                // A column: each toy as wide as the strip allows, no taller than it is wide.
                let room = Rect::new(Vec2::new(area.min.x, area.min.y + along), Vec2::splat(area.size.x));
                let fit = Fit::new(w, h, room, Some(scale), hex_px);
                let top = area.min.y + along;
                let fit = Fit { origin: Vec2::new(fit.origin.x, top.floor()), ..fit };
                let slot = Rect::new(Vec2::new(area.min.x, top), Vec2::new(area.size.x, fit.span.y + label));
                along += slot.size.y + gap;
                (fit, slot)
            } else {
                // A row: each toy as tall as the strip allows, less its label.
                let tall = area.size.y - label;
                let room = Rect::new(Vec2::new(area.min.x + along, area.min.y), Vec2::new(tall * 1.6, tall));
                let fit = Fit::new(w, h, room, Some(scale), hex_px);
                let left = area.min.x + along;
                let wide = fit.span.x.max(s.em(5.0));
                let origin = Vec2::new((left + (wide - fit.span.x) * 0.5).floor(), fit.origin.y);
                let fit = Fit { origin, ..fit };
                let slot = Rect::new(Vec2::new(left, area.min.y), Vec2::new(wide, area.size.y));
                along += wide + gap;
                (fit, slot)
            }
        })
        .collect()
}

/// How far the strip can scroll: its length past the area's.
pub fn scroll_room(slots: &[(Fit, Rect)], area: Rect, portrait: bool, scroll: f32) -> f32 {
    let Some(last) = slots.last() else { return 0.0 };
    let end = if portrait { last.1.max().x - area.max().x } else { last.1.max().y - area.max().y };
    (end + scroll).max(0.0)
}

const INK: u32 = 0xeeeef2;

/// Labels, and a veil with PLAY over paused toys.
pub fn draw(shapes: &mut Shapes, toys: &[Toy], slots: &[(Fit, Rect)], area: Rect, s: Screen) {
    shapes.view = View::PIXELS;
    for (toy, (fit, slot)) in toys.iter().zip(slots) {
        let r = fit.rect();
        if r.max().x < area.min.x || r.min.x > area.max().x || r.max().y < area.min.y || r.min.y > area.max().y {
            continue;
        }
        let label = Vec2::new(slot.center().x, slot.max().y - s.em(LABEL * 0.5));
        if area.contains(label) {
            shapes.text(label, s.em(0.75), toy.scene.name, rgba(INK, if toy.playing { 0.9 } else { 0.55 }));
        }
        if !toy.playing && area.contains(r.min) && area.contains(r.max() - 1.0) {
            shapes.rect(r.center(), r.size, rgba(0x08090c, 0.45));
            let pill = Vec2::new(s.em(3.6), s.em(1.6));
            shapes
                .rounded_rect(r.center(), pill, pill.y * 0.5, rgba(0x1b1f2a, 0.9))
                .outline(s.em(0.06), rgba(INK, 0.4));
            shapes.text(r.center(), s.em(0.75), "PLAY", rgb(INK));
        }
    }
}
