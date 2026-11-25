//! The brush: the player's input, and the only thing that spawns or erases particles. It works
//! between phases and keeps clear of cells engaged in a contract, so it never breaks one.

use crate::{Material, State, World};

impl World {
    /// The brush, the one source and sink of particles. It spawns a particle (air, water, sand,
    /// slate, bedrock) only into a cell holding nothing, never over another; erasing (painting
    /// vacuum) empties anything but bedrock. A
    /// painted particle intends to move down (`State::holding`). Cells engaged in a contract are
    /// left alone. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        let here = self.material(x, y);
        let allowed =
            here != material && here != Material::Bedrock && (material == Material::Vacuum || here == Material::Vacuum);
        if allowed && !self.engaged(x, y) {
            self.set(x, y, State::holding(material));
        }
    }

    /// One brush stamp: `paint` every cell whose centre lies within `radius` of `(cx, cy)`, in
    /// cell units, where a cell's centre is `(x, y + 0.5)` in odd columns and `(x, y)` in even ones.
    pub fn stamp(&mut self, cx: f32, cy: f32, radius: f32, material: Material) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                let (dx, dy) = (x as f32 - cx, y as f32 + 0.5 * (x & 1) as f32 - cy);
                if dx * dx + dy * dy <= radius * radius {
                    self.paint(x, y, material);
                }
            }
        }
    }
}
