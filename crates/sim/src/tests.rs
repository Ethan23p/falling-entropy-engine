//! Tests, named after the primitives and policies they pin. v01.00 starts with the invariants the
//! scaffolding keeps: geometry, randomness, synchrony, the brush, the encoding and conservation.

use crate::*;

/// A scattering of sand over the top half: a deterministic pseudo-random fill.
fn scattered(w: i32, h: i32) -> World {
    let mut world = World::new(w, h);
    for y in 0..h / 2 {
        for x in 0..w {
            if hash(x, y, 0, 99).is_multiple_of(3) {
                world.paint(x, y, Material::Sand);
            }
        }
    }
    world
}

fn sand_at(world: &World) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for y in 0..world.height() {
        for x in 0..world.width() {
            if world.material(x, y) == Material::Sand {
                out.push((x, y));
            }
        }
    }
    out
}

// Geometry

#[test]
fn geometry_neighbor_of_neighbor_is_self() {
    for x in 1..5 {
        for y in 1..5 {
            for d in Dir::ALL {
                let (nx, ny) = neighbor(x, y, d);
                assert_eq!(neighbor(nx, ny, d.opposite()), (x, y), "({x},{y}) {d:?}");
            }
        }
    }
}

#[test]
fn geometry_six_distinct_neighbors() {
    for x in [2, 3] {
        let mut seen: Vec<_> = Dir::ALL.iter().map(|&d| neighbor(x, 2, d)).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 6);
    }
}

#[test]
fn geometry_odd_columns_sit_half_a_cell_lower() {
    // From an even column, down-right stays in the same row; from an odd one it drops a row.
    assert_eq!(neighbor(2, 2, Dir::DownRight), (3, 2));
    assert_eq!(neighbor(3, 2, Dir::DownRight), (4, 3));
}

// Randomness

#[test]
fn hash_is_deterministic_and_varies() {
    assert_eq!(hash(3, 4, 5, 6), hash(3, 4, 5, 6));
    assert_ne!(hash(3, 4, 5, 6), hash(3, 4, 6, 6));
    assert_ne!(hash(3, 4, 5, 6), hash(4, 3, 5, 6));
    assert_ne!(hash(3, 4, 5, 6), hash(3, 4, 5, 7));
}

#[test]
fn hash_low_bit_is_balanced() {
    let ones: u32 = (0..10_000).map(|i| hash(i % 100, i / 100, 7, 1) & 1).sum();
    assert!((4_700..5_300).contains(&ones), "{ones}");
}

// Synchrony

#[test]
fn synchrony_result_is_independent_of_evaluation_order() {
    let mut a = scattered(20, 20);
    let mut b = a.clone();
    let cells: Vec<_> = (0..20).flat_map(|y| (0..20).map(move |x| (x, y))).collect();
    for phase in 0..60 {
        a.step();
        // Evaluate b's cells in a scrambled order, with every cell updated once.
        let mut order = cells.clone();
        order.sort_by_key(|&(x, y)| hash(x, y, phase, 5));
        for (x, y) in order {
            assert!(b.update(x, y), "every cell can advance in a full sweep");
        }
        assert_eq!(a, b, "phase {phase}");
    }
}

#[test]
fn synchrony_a_cell_waits_for_a_neighbor_behind() {
    let mut w = scattered(5, 5);
    assert!(w.update(2, 2));
    assert!(!w.update(2, 2), "(2, 2) is a phase ahead of its neighbors now");
    assert_eq!(w.phase(2, 2), 1);
    assert_eq!(w.phase(2, 1), 0);
}

#[test]
fn synchrony_advances_at_most_one_phase_ahead() {
    let mut w = scattered(10, 10);
    for _ in 0..5 {
        w.update(4, 4);
    }
    assert_eq!(w.phase(4, 4), 1);
}

#[test]
fn synchrony_kept_back_values_serve_a_neighbor_behind() {
    // Advancing a cell early must not change what its neighbors compute.
    let mut a = scattered(12, 12);
    for _ in 0..7 {
        a.step();
    }
    let mut b = a.clone();
    a.step();
    b.update(5, 3);
    b.update(5, 4);
    b.update(6, 4);
    for y in 0..12 {
        for x in 0..12 {
            if b.phase(x, y) == 7 {
                assert!(b.update(x, y));
            }
        }
    }
    assert_eq!(a, b);
}

#[test]
fn synchrony_a_ragged_front_of_phases_gives_the_same_world() {
    // Cells advance whenever their neighbors let them, in a hashed order, so the phase front is
    // ragged; once every cell reaches the same phase, the world is the one lockstep gives.
    let mut a = scattered(16, 16);
    let mut b = a.clone();
    for _ in 0..40 {
        a.step();
    }
    let mut tick = 0;
    while (0..16).any(|y| (0..16).any(|x| b.phase(x, y) < 40)) {
        let (x, y) = ((hash(tick, 0, 0, 22) % 16) as i32, (hash(tick, 1, 0, 22) % 16) as i32);
        if b.phase(x, y) < 40 {
            b.update(x, y);
        }
        tick += 1;
    }
    assert!(b.uniform());
    assert_eq!(a, b);
}

// Conservation

#[test]
fn conservation_sand_count_never_changes() {
    let mut w = scattered(20, 16);
    let sand = w.count(Material::Sand);
    for _ in 0..100 {
        w.step();
        assert_eq!(w.count(Material::Sand), sand);
    }
}

// Brush

#[test]
fn brush_paints_and_erases() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Sand);
    assert_eq!(w.count(Material::Sand), 1);
    w.paint(1, 1, Material::Air);
    assert_eq!(w.count(Material::Sand), 0);
}

#[test]
fn brush_paints_only_into_air() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Sand);
    w.paint(1, 1, Material::Bedrock);
    assert_eq!(w.material(1, 1), Material::Sand, "bedrock does not replace sand");
    w.paint(2, 2, Material::Bedrock);
    w.paint(2, 2, Material::Sand);
    assert_eq!(w.material(2, 2), Material::Bedrock, "sand does not replace bedrock");
}

#[test]
fn brush_cannot_erase_bedrock() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Bedrock);
    w.paint(1, 1, Material::Air);
    w.stamp(1.0, 1.0, 3.0, Material::Air);
    assert_eq!(w.material(1, 1), Material::Bedrock);
}

#[test]
fn brush_stamp_covers_a_disc_of_centres() {
    let mut w = World::new(9, 9);
    w.stamp(4.0, 4.0, 1.2, Material::Sand);
    // Even column 4: (4,3), (4,4), (4,5); odd columns 3 and 5 sit half a cell lower: rows 3 and 4.
    assert_eq!(sand_at(&w), vec![(3, 3), (4, 3), (5, 3), (3, 4), (4, 4), (5, 4), (4, 5)]);
}

#[test]
fn brush_keeps_the_cells_phase() {
    let mut w = World::new(4, 4);
    w.step();
    w.step();
    w.paint(1, 1, Material::Sand);
    assert_eq!(w.phase(1, 1), 2);
    assert!(w.uniform());
}

// Encoding

#[test]
fn encoding_state_round_trips() {
    for material in [Material::Air, Material::Sand, Material::Bedrock] {
        let s = State::holding(material);
        assert_eq!(State::decode(s.encode()), s);
        assert!(s.encode() < 1 << 19, "19 bits");
    }
}

#[test]
fn encoding_a_cell_keeps_a_word_per_phase_parity() {
    // The state of phase p goes in word p % 2, tagged with p, so the word a neighbor at p reads
    // stays put until p + 2.
    let mut w = World::new(3, 3);
    w.paint(1, 0, Material::Sand);
    const { assert!(STATE_MASK == (1 << 19) - 1 && PHASE_MASK == (1 << 13) - 1) };
    w.step();
    let words = w.words();
    let (even, odd) = (words[2], words[3]);
    assert_eq!((even >> 19, odd >> 19), (0, 1));
    assert_eq!(State::decode(even & STATE_MASK), State::holding(Material::Sand));
    assert_eq!((w.kept(1, 0), w.cell(1, 0)), (State::holding(Material::Sand), State::holding(Material::Sand)));
}

#[test]
fn encoding_the_phase_wraps() {
    let mut w = World::new(3, 3);
    for _ in 0..PHASE_MASK + 3 {
        w.step();
    }
    assert!(w.uniform());
    assert_eq!(w.phase(1, 1), 2);
}
