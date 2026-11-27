//! The brush: the player's input, and the only thing that spawns or erases particles. It works
//! between phases and keeps clear of cells engaged in a contract, so it never breaks one.
//!
//! It stamps polyhexes: one to four hexes joined edge to edge, in one of twelve shapes. Each is
//! scaled so every shape at a size covers the same number of cells: the volume is normalized.

use crate::lattice::{from_axial, to_axial};
use crate::{Material, State, World};

impl World {
    /// The brush, the one source and sink of particles. It spawns a particle (air, water, sand,
    /// slate, bedrock) only into a cell holding insubstantial mass, air or vacuum, never over a
    /// substantial particle; the air there ceases to exist. Erasing (painting vacuum) empties
    /// anything but bedrock. A painted particle intends to move down (`State::holding`). Cells
    /// engaged in a contract are left alone. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        if self.accepts(x, y, material) {
            self.set(x, y, State::holding(material));
        }
    }

    /// Whether the cell would take `material` from the brush now (`paint`).
    fn accepts(&self, x: i32, y: i32, material: Material) -> bool {
        let here = self.material(x, y);
        let allowed =
            here != material && here != Material::Bedrock && (material == Material::Vacuum || !here.is_substantial());
        allowed && !self.engaged(x, y)
    }

    /// A disc stamp (scene scripts): offers its material (`offer`) to every cell whose centre lies
    /// within `radius` of `(cx, cy)`, in cell units, where a cell's centre is `(x, y + 0.5)` in odd
    /// columns and `(x, y)` in even ones.
    pub fn stamp(&mut self, cx: f32, cy: f32, radius: f32, material: Material) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                let (dx, dy) = (x as f32 - cx, y as f32 + 0.5 * (x & 1) as f32 - cy);
                if dx * dx + dy * dy <= radius * radius {
                    self.offer(x, y, material);
                }
            }
        }
    }

    /// The brush's stamp: offers its material to every cell of a placed tetrahex on the lattice.
    pub fn stamp_polyhex(&mut self, t: &Placed, material: Material) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                if t.covers(x, y) {
                    self.offer(x, y, material);
                }
            }
        }
    }

    /// The brush offers `material` to the cell: the offer waits on the cell until the cell is free
    /// of contracts, then paints it (`settle`). A later offer replaces one still waiting.
    fn offer(&mut self, x: i32, y: i32, material: Material) {
        let i = (y * self.width() + x) as usize;
        self.brush[i] = (material as u32 + 1) | BRUSH_WAIT << 8;
    }

    /// Settles the brush's waiting offers, before a phase: a cell that takes the offer now (free of
    /// contracts, and holding what the brush may paint over) is painted and the offer is done;
    /// otherwise the offer keeps waiting, for at most `BRUSH_WAIT` phases. Each offer paints its
    /// cell at most once, so a stamp never paints more than its volume.
    pub fn settle(&mut self) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                let i = (y * self.width() + x) as usize;
                let offer = self.brush[i];
                if offer == 0 {
                    continue;
                }
                let material = Material::ALL[((offer & 0xff) - 1) as usize];
                if self.accepts(x, y, material) {
                    self.set(x, y, State::holding(material));
                    self.brush[i] = 0;
                } else {
                    let wait = (offer >> 8) - 1;
                    self.brush[i] = if wait == 0 { 0 } else { (offer & 0xff) | wait << 8 };
                }
            }
        }
    }
}

/// The most phases a brush offer waits on a cell engaged in a contract before it lapses. Air among
/// air proposes nearly every phase, so it's engaged most of the time: three in four of its cells
/// come free within 16 phases, nearly all within 64 (a quarter second at 240 phases a second).
pub const BRUSH_WAIT: u32 = 96;

/// The twelve polyhexes of one to four hexes: every shape of hexes joined edge to edge, up to turns
/// and mirroring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Polyhex {
    Hex,
    Pair,
    /// Three around a corner.
    Triangle,
    /// Three with a bend.
    Bend,
    /// Three in a row.
    Line,
    // The seven tetrahexes, by their usual names.
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

impl Polyhex {
    pub const ALL: [Polyhex; 12] = [
        Polyhex::Hex,
        Polyhex::Pair,
        Polyhex::Triangle,
        Polyhex::Bend,
        Polyhex::Line,
        Polyhex::Bee,
        Polyhex::Bar,
        Polyhex::Pistol,
        Polyhex::Propeller,
        Polyhex::Worm,
        Polyhex::Arch,
        Polyhex::Wave,
    ];

    /// Its hexes in axial coordinates, built from three neighbor steps a sixth of a turn apart:
    /// `a` down-right, `b` down, `c` down-left.
    pub fn hexes(self) -> Vec<(i32, i32)> {
        let (a, b, c) = ((1, 0), (0, 1), (-1, 1));
        let add = |p: (i32, i32), q: (i32, i32)| (p.0 + q.0, p.1 + q.1);
        let o = (0, 0);
        match self {
            Polyhex::Hex => vec![o],
            Polyhex::Pair => vec![o, a],
            Polyhex::Triangle => vec![o, a, b],
            Polyhex::Bend => vec![o, a, add(a, b)],
            Polyhex::Line => vec![o, a, add(a, a)],
            // Two touching triangles: every hex touches two or three others.
            Polyhex::Bee => vec![o, a, b, add(a, b)],
            Polyhex::Bar => vec![o, a, add(a, a), add(add(a, a), a)],
            // A triangle with a bar off one corner.
            Polyhex::Pistol => vec![o, a, b, add(a, a)],
            // One hex with three arms, every other side.
            Polyhex::Propeller => vec![o, a, c, (0, -1)],
            // A bar of three, then a bend.
            Polyhex::Worm => vec![o, a, add(a, a), add(add(a, a), b)],
            // Two bends the same way.
            Polyhex::Arch => vec![o, a, add(a, b), add(add(a, b), c)],
            // Two bends, opposite ways.
            Polyhex::Wave => vec![o, a, add(a, b), add(add(a, b), a)],
        }
    }
}

/// The brush's volume at a size: four hexes of radius `size` (`4 (3 R^2 + 3 R + 1)` cells), the
/// size of a tetrahex made of four superhexes. Every shape at a size covers this many cells.
pub fn volume(size: i32) -> u32 {
    let r = size.clamp(0, MAX_SIZE) as u32;
    4 * (3 * r * r + 3 * r + 1)
}

/// The largest brush size.
pub const MAX_SIZE: i32 = 16;

/// A polyhex as the brush stamps it, scaled so it covers exactly `volume(size)` cells: each of its
/// `pieces` hexes grows into a hex-shaped piece, the pieces spaced as superhexes tile, and the
/// cells nearest the pieces' centres, up to the volume, make the shape. Pieces take the whole radius
/// under their share of the volume, and grow a partial ring, sides first, to make it up; at whole
/// radii (a tetrahex at any size) the pieces are exactly superhexes.
///
/// A cell's key orders it (`key`): its hex distance to the nearest piece centre, then its squared
/// straight-line distance, then a tie-break hashed from its place in the shape. The shape is the
/// cells whose key is at most `threshold`; the CPU finds that threshold once, and the GPU shaders
/// only compare.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placed {
    /// The axial cell the shape is centred on.
    pub origin: (i32, i32),
    /// The pieces' centres, axial cells relative to `origin`.
    pub centres: [(i32, i32); 4],
    pub pieces: u32,
    pub threshold: (u32, u32, u32),
    pub volume: u32,
}

/// The key of the cell at axial offset `(q, r)` from a shape's origin, against `centres`: its hex
/// distance to the nearest centre (doubled), then its squared straight-line distance, then a tie-break unique to the cell.
pub fn key(centres: &[(i32, i32)], (q, r): (i32, i32)) -> (u32, u32, u32) {
    let mut best = (u32::MAX, u32::MAX);
    for &(cq, cr) in centres {
        let (dq, dr) = (q - cq, r - cr);
        let hex = (dq.abs() + dr.abs() + (dq + dr).abs()) as u32;
        // The hex lattice's straight-line norm, squared, in axial coordinates.
        let line = (dq * dq + dq * dr + dr * dr) as u32;
        best = best.min((hex, line));
    }
    // Hashed high bits, so ties break without a bias; the offset in the low bits keeps every key
    // near a shape unique, so exactly `volume` cells sit at or under the threshold.
    let tie = (crate::hash(q, r, 0, SALT_TIE) & 0xffff) << 16 | ((q & 255) as u32) << 8 | (r & 255) as u32;
    (best.0, best.1, tie)
}

impl Placed {
    /// `shape`, turned `turns` sixths clockwise and mirrored if `mirror`, scaled to `volume(size)`
    /// cells and centred on the cell `(x, y)`.
    pub fn new(shape: Polyhex, turns: u32, mirror: bool, size: i32, (x, y): (i32, i32)) -> Placed {
        let volume = volume(size);
        let hexes = shape.hexes();
        let n = hexes.len() as f64;
        // Each piece's radius, so the pieces together hold the volume: n (3 rho^2 + 3 rho + 1) = V.
        let rho = ((9.0 - 12.0 * (1.0 - volume as f64 / n)).sqrt() - 3.0) / 6.0;
        // Pieces of the whole radius R under rho sit where superhexes of that radius tile, centred
        // on cells: the unit steps (1, 0) and (0, 1) go to (2 R + 1, -R) and (R, R + 1), a linear
        // map, so adjacent hexes stay adjacent pieces. The threshold then grows each piece's outer
        // ring, sides first, to the volume.
        let big = (rho + 1e-6).floor() as i32;
        let scale = |(q, r): (i32, i32)| (q * (2 * big + 1) + r * big, -q * big + r * (big + 1));
        let placed: Vec<(i32, i32)> = hexes
            .iter()
            .map(|&h| {
                let mut h = if mirror { (h.1, h.0) } else { h };
                for _ in 0..turns % 6 {
                    h = turn(h);
                }
                scale(h)
            })
            .collect();
        // Centred on the shape's mean, rounded to a cell.
        let (sq, sr) = placed.iter().fold((0, 0), |s, c| (s.0 + c.0, s.1 + c.1));
        let (mq, mr) = ((sq as f64 / n).round() as i32, (sr as f64 / n).round() as i32);
        let mut centres = [(0, 0); 4];
        for (k, &(q, r)) in placed.iter().enumerate() {
            centres[k] = (q - mq, r - mr);
        }
        let pieces = hexes.len() as u32;
        // The threshold: the key of the volume-th nearest cell.
        let reach = big + 2;
        let mut keys = Vec::new();
        for &(cq, cr) in &centres[..pieces as usize] {
            for dq in -reach..=reach {
                for dr in -reach..=reach {
                    if (dq + dr).abs() <= reach {
                        keys.push(((cq + dq, cr + dr), key(&centres[..pieces as usize], (cq + dq, cr + dr))));
                    }
                }
            }
        }
        keys.sort_unstable();
        keys.dedup();
        let mut keys: Vec<(u32, u32, u32)> = keys.into_iter().map(|(_, k)| k).collect();
        keys.sort_unstable();
        let threshold = keys[volume as usize - 1];
        Placed { origin: to_axial(x, y), centres, pieces, threshold, volume }
    }

    /// A shape, turn and mirroring chosen by `seed`, each of the twelve shapes equally likely.
    pub fn random(seed: u32, size: i32, at: (i32, i32)) -> Placed {
        let h = crate::hash(seed as i32, 0, 0, SALT_SHAPE);
        let shape = Polyhex::ALL[(h % 12) as usize];
        Placed::new(shape, (h / 12) % 6, (h / 72) & 1 == 1, size, at)
    }

    /// The same shape, centred on the cell `(x, y)` instead.
    pub fn moved_to(self, (x, y): (i32, i32)) -> Placed {
        Placed { origin: to_axial(x, y), ..self }
    }

    /// Whether the cell `(x, y)` is part of it.
    pub fn covers(&self, x: i32, y: i32) -> bool {
        let (q, r) = to_axial(x, y);
        key(&self.centres[..self.pieces as usize], (q - self.origin.0, r - self.origin.1)) <= self.threshold
    }

    /// The cells it covers, on or off the lattice: always `volume(size)` of them.
    pub fn volume(&self) -> usize {
        self.volume as usize
    }

    /// Every cell it covers, as `(x, y)`, on or off the lattice.
    pub fn cells(&self) -> Vec<(i32, i32)> {
        let reach = self.centres.iter().map(|c| c.0.abs().max(c.1.abs())).max().unwrap_or(0) + 40;
        let mut out = Vec::with_capacity(self.volume());
        for dq in -reach..=reach {
            for dr in -reach..=reach {
                let (q, r) = (self.origin.0 + dq, self.origin.1 + dr);
                let (x, y) = from_axial(q, r);
                if self.covers(x, y) {
                    out.push((x, y));
                }
            }
        }
        out
    }
}

/// The salts of the brush's shape choice and of its cells' tie-break.
const SALT_SHAPE: u32 = 0x7e7a;
const SALT_TIE: u32 = 0x7e7b;
