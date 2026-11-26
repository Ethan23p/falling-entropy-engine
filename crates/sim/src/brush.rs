//! The brush: the player's input, and the only thing that spawns or erases particles. It works
//! between phases and keeps clear of cells engaged in a contract, so it never breaks one.
//!
//! It stamps tetrahexes: four hexes joined edge to edge, in one of seven shapes. A larger size
//! makes each of the four a hex of that radius (a superhex), and those tile the lattice exactly,
//! so every shape at a size covers the same number of cells: the volume is normalized.

use crate::lattice::{from_axial, hex_distance, to_axial};
use crate::{Material, State, World};

impl World {
    /// The brush, the one source and sink of particles. It spawns a particle (air, water, sand,
    /// slate, bedrock) only into a cell holding insubstantial mass, air or vacuum, never over a
    /// substantial particle; the air there ceases to exist. Erasing (painting vacuum) empties
    /// anything but bedrock. A painted particle intends to move down (`State::holding`). Cells
    /// engaged in a contract are left alone. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        let here = self.material(x, y);
        let allowed =
            here != material && here != Material::Bedrock && (material == Material::Vacuum || !here.is_substantial());
        if allowed && !self.engaged(x, y) {
            self.set(x, y, State::holding(material));
        }
    }

    /// A disc stamp (scene scripts): `paint` every cell whose centre lies within `radius` of
    /// `(cx, cy)`, in cell units, where a cell's centre is `(x, y + 0.5)` in odd columns and `(x, y)`
    /// in even ones.
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

    /// The brush's stamp: `paint` every cell of a placed tetrahex that lies on the lattice.
    pub fn stamp_tetrahex(&mut self, t: &Placed, material: Material) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                if t.covers(x, y) {
                    self.paint(x, y, material);
                }
            }
        }
    }
}

/// The seven tetrahexes, by their usual names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tetrahex {
    Bee,
    Bar,
    Pistol,
    Propeller,
    Worm,
    Arch,
    Wave,
}

/// A sixth of a turn clockwise, in axial coordinates.
fn turn((q, r): (i32, i32)) -> (i32, i32) {
    (-r, q + r)
}

impl Tetrahex {
    pub const ALL: [Tetrahex; 7] = [
        Tetrahex::Bee,
        Tetrahex::Bar,
        Tetrahex::Pistol,
        Tetrahex::Propeller,
        Tetrahex::Worm,
        Tetrahex::Arch,
        Tetrahex::Wave,
    ];

    /// Its four hexes in axial coordinates, built from three neighbor steps a sixth of a turn
    /// apart: `a` down-right, `b` down, `c` down-left.
    pub fn hexes(self) -> [(i32, i32); 4] {
        let (a, b, c) = ((1, 0), (0, 1), (-1, 1));
        let add = |p: (i32, i32), q: (i32, i32)| (p.0 + q.0, p.1 + q.1);
        let o = (0, 0);
        match self {
            // Two touching triangles: every hex touches two or three others.
            Tetrahex::Bee => [o, a, b, add(a, b)],
            Tetrahex::Bar => [o, a, add(a, a), add(add(a, a), a)],
            // A triangle with a bar off one corner.
            Tetrahex::Pistol => [o, a, b, add(a, a)],
            // One hex with three arms, every other side.
            Tetrahex::Propeller => [o, a, c, (0, -1)],
            // A bar of three, then a bend.
            Tetrahex::Worm => [o, a, add(a, a), add(add(a, a), b)],
            // Two bends the same way.
            Tetrahex::Arch => [o, a, add(a, b), add(add(a, b), c)],
            // Two bends, opposite ways.
            Tetrahex::Wave => [o, a, add(a, b), add(add(a, b), a)],
        }
    }
}

/// A tetrahex as the brush stamps it: the centres of its four superhexes (axial coordinates on the
/// lattice) and their radius. It covers `Placed::volume` cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placed {
    pub centres: [(i32, i32); 4],
    pub radius: i32,
}

impl Placed {
    /// `shape`, turned `turns` sixths clockwise and mirrored if `mirror`, scaled so each hex is a
    /// superhex of `radius`, and centred on the cell `(x, y)`.
    pub fn new(shape: Tetrahex, turns: u32, mirror: bool, radius: i32, (x, y): (i32, i32)) -> Placed {
        let radius = radius.max(0);
        // Superhexes of radius R tile with neighbors at (2R + 1, -R) and (R, R + 1), the two unit
        // steps (1, 0) and (0, 1) scaled; a linear map, so the shape keeps its adjacencies.
        let scale = |(q, r): (i32, i32)| (q * (2 * radius + 1) + r * radius, -q * radius + r * (radius + 1));
        let mut centres = shape.hexes().map(|mut h| {
            if mirror {
                h = (h.1, h.0);
            }
            for _ in 0..turns % 6 {
                h = turn(h);
            }
            scale(h)
        });
        // Centre the shape: its mean, rounded to a cell, goes to (x, y).
        let (mq, mr) = centres.iter().fold((0.0, 0.0), |s, c| (s.0 + c.0 as f32 / 4.0, s.1 + c.1 as f32 / 4.0));
        let mid = round_axial(mq, mr);
        let at = to_axial(x, y);
        for c in &mut centres {
            *c = (c.0 - mid.0 + at.0, c.1 - mid.1 + at.1);
        }
        Placed { centres, radius }
    }

    /// A shape, turn and mirroring chosen by `seed`.
    pub fn random(seed: u32, radius: i32, at: (i32, i32)) -> Placed {
        let h = crate::hash(seed as i32, 0, 0, SALT_SHAPE);
        let shape = Tetrahex::ALL[(h % 7) as usize];
        Placed::new(shape, (h / 7) % 6, (h / 42) & 1 == 1, radius, at)
    }

    /// Whether the cell `(x, y)` is part of it.
    pub fn covers(&self, x: i32, y: i32) -> bool {
        let p = to_axial(x, y);
        self.centres.iter().any(|&c| hex_distance(p, c) <= self.radius)
    }

    /// The cells it covers, on or off the lattice: four superhexes, whatever the shape.
    pub fn volume(&self) -> usize {
        let r = self.radius as usize;
        4 * (3 * r * r + 3 * r + 1)
    }

    /// Every cell it covers, as `(x, y)`, on or off the lattice.
    pub fn cells(&self) -> Vec<(i32, i32)> {
        let r = self.radius;
        let mut out = Vec::with_capacity(self.volume());
        for &(cq, cr) in &self.centres {
            for dq in -r..=r {
                for dr in (-r).max(-dq - r)..=r.min(-dq + r) {
                    out.push(from_axial(cq + dq, cr + dr));
                }
            }
        }
        out
    }
}

/// The salt of the brush's shape choice.
const SALT_SHAPE: u32 = 0x7e7a;

/// The axial cell nearest fractional axial `(q, r)`.
fn round_axial(q: f32, r: f32) -> (i32, i32) {
    let s = -q - r;
    let (mut rq, mut rr, rs) = (q.round(), r.round(), s.round());
    let (dq, dr, ds) = ((rq - q).abs(), (rr - r).abs(), (rs - s).abs());
    if dq > dr && dq > ds {
        rq = -rr - rs;
    } else if dr > ds {
        rr = -rq - rs;
    }
    (rq as i32, rr as i32)
}
