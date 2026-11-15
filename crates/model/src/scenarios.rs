//! v01.00's proving scenarios: a column 1 wide, a column 2 wide, and rings flowing by directed
//! contracts. Each builds a world; [`run`] steps one with every invariant checked each phase.

use crate::{Material, Shape, World};

/// A column 10 high and `width` wide on the hex lattice, its top three rows `material`, air below.
/// Beyond the edge is bedrock.
pub fn column(width: i32, material: Material) -> World {
    let mut w = World::new(Shape::Hex, width, 10);
    for y in 0..3 {
        for x in 0..width {
            w.place(x, y, material, (0, 0));
        }
    }
    w
}

/// How a ring starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Start {
    /// Full of water at rest.
    Full,
    /// Flowing clockwise: every other cell has just given its water to the cell ahead and, in the
    /// same update, received the water from the cell behind. Each unit points the way it moves.
    Flowing,
}

/// A ring one cell wide with bedrock inside and out, full of water and
/// started as `start` says. Returns the world and the ring's cells in order, clockwise from the
/// top-left.
pub fn ring(shape: Shape, start: Start) -> (World, Vec<usize>) {
    let mut w = World::new(shape, 7, 7);
    let on_ring = |x: i32, y: i32| match shape {
        // The cells two steps from the centre: 12 cells, with six 60-degree corners.
        Shape::Hex => hex_distance((x, y), (3, 3)) == 2,
    };
    let cells: Vec<usize> = (0..w.len()).filter(|&i| on_ring(w.xy(i).0, w.xy(i).1)).collect();
    // Walk the ring clockwise from its top-left cell, heading right.
    let start_cell = *cells.iter().min_by_key(|&&i| (w.xy(i).1, w.xy(i).0)).unwrap();
    let mut order = vec![start_cell];
    while order.len() < cells.len() {
        let here = *order.last().unwrap();
        let next = (0..shape.slots())
            .filter_map(|s| w.neighbor(here, s))
            .filter(|n| cells.contains(n) && !order.contains(n))
            .max_by_key(|&n| (w.xy(n).0, -w.xy(n).1))
            .expect("a ring cell has an unvisited ring neighbor");
        order.push(next);
    }
    for i in 0..w.len() {
        let (x, y) = w.xy(i);
        w.place(x, y, Material::Bedrock, (0, 0));
    }
    for &i in &order {
        let (x, y) = w.xy(i);
        w.place(x, y, Material::Water, (0, 0));
    }
    if start == Start::Flowing {
        // Every other cell has just given its water onward and, in the same update, received the
        // water from behind; it offers that on. The cells between hold water already given onward,
        // whose acceptance they read next. Every cell shows water; the vacuum is only ever in
        // transit, so half the units are shown twice.
        let n = order.len();
        let slot =
            |w: &World, from: usize, to: usize| (0..shape.slots()).find(|&s| w.neighbor(from, s) == Some(to)).unwrap();
        let fwd = |w: &World, k: usize| slot(w, order[k], order[(k + 1) % n]);
        let unit = |w: &World, k: usize| crate::Matter { velocity: shape.dir(fwd(w, k)), ..w.cell(order[k]).matter };
        let mut seeds = Vec::new();
        for k in 0..n {
            let (i, back) = (order[k], (k + n - 1) % n);
            let from_behind = slot(&w, i, order[back]);
            if k % 2 == 1 {
                let now = crate::State::holding(unit(&w, k));
                let mut kept = now;
                kept.offers[fwd(&w, k)] = true;
                kept.verdicts[from_behind] = crate::Verdict::Accept;
                seeds.push((i, now, kept));
            } else {
                // Its velocity, if it pointed into bedrock here, turned the way it offers.
                let mut received = unit(&w, back);
                if w.neighbor(i, fwd(&w, back)).is_none_or(|b| w.is_bedrock(b)) {
                    received.velocity = shape.dir(fwd(&w, k));
                }
                let mut now = crate::State::holding(received);
                now.offers[fwd(&w, k)] = true;
                now.verdicts[from_behind] = crate::Verdict::Accept;
                seeds.push((i, now, crate::State::holding(unit(&w, (k + 1) % n))));
            }
        }
        for (i, now, kept) in seeds {
            w.seed(i, now, kept);
        }
    }
    (w, order)
}

/// Steps on the hex lattice between two positions, through cube coordinates.
pub fn hex_distance(a: (i32, i32), b: (i32, i32)) -> i32 {
    let cube = |(x, y): (i32, i32)| (x, y - (x - (x & 1)) / 2);
    let ((aq, ar), (bq, br)) = (cube(a), cube(b));
    let (dq, dr) = (aq - bq, ar - br);
    (dq.abs() + dr.abs() + (dq + dr).abs()) / 2
}

/// Steps `world` `phases` times, checking after every phase the contract rules and conservation on
/// published state, and calling `each` on the result.
pub fn run(world: &mut World, phases: u32, mut each: impl FnMut(&World)) -> Result<(), String> {
    let expected = world.inventory();
    for _ in 0..phases {
        world.step();
        world.audit()?;
        world.conserves(&expected)?;
        each(world);
    }
    Ok(())
}

/// Whether any matter moved this phase: an acceptance published anywhere. Matter at rest may still
/// be offering and being refused.
pub fn moved(world: &World) -> bool {
    world.cells().any(|i| world.cell(i).accepted().is_some())
}

/// Where each unit of matter on a ring is, by its place along the ring, sorted by id.
pub fn ring_places(world: &World, ring: &[usize]) -> Vec<(u16, usize)> {
    let mut places: Vec<(u16, usize)> = ring
        .iter()
        .enumerate()
        .map(|(k, &i)| (world.owned(i), k))
        .filter(|(m, _)| !m.is_air())
        .map(|(m, k)| (m.id, k))
        .collect();
    places.sort_unstable();
    places
}

/// Writes a dump to `target/model/<name>.txt`, for looking at.
pub fn dump(name: &str, text: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/model");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{name}.txt")), text);
}

/// One frame of a dump: the phase, what moved, and the world with intents.
pub fn frame(world: &World) -> String {
    let moves = world.cells().filter(|&i| world.cell(i).accepted().is_some()).count();
    let offers = world.cells().filter(|&i| world.cell(i).offering()).count();
    let rejected: usize =
        world.cells().map(|i| world.cell(i).verdicts.iter().filter(|&&v| v == crate::Verdict::Reject).count()).sum();
    format!(
        "phase {}: {offers} offers, {moves} accepted, {rejected} rejected, {} in transit (shown twice)\n{}",
        world.now(),
        world.in_transit(),
        world.ascii_intents()
    )
}
