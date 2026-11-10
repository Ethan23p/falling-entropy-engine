//! Rule tests, named after the primitives and policies they pin.

use crate::*;

/// A world with sand at each `(x, y)`.
fn world(w: i32, h: i32, sand: &[(i32, i32)]) -> World {
    let mut world = World::new(w, h);
    for &(x, y) in sand {
        world.paint(x, y, Material::Sand);
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

// Gravity

#[test]
fn gravity_downhill_is_down_and_the_lower_diagonals() {
    for x in [2, 3] {
        let mut downhill: Vec<_> = Dir::ALL.into_iter().filter(|&d| drop(x, 2, d) > 0).collect();
        downhill.sort_by_key(|&d| d as u8);
        assert_eq!(downhill, vec![Dir::DownRight, Dir::Down, Dir::DownLeft]);
    }
}

#[test]
fn gravity_straight_down_is_steepest() {
    for x in [2, 3] {
        assert!(drop(x, 2, Dir::Down) > drop(x, 2, Dir::DownLeft));
        assert_eq!(drop(x, 2, Dir::DownLeft), drop(x, 2, Dir::DownRight));
    }
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

// Offer, accept, transfer

#[test]
fn offer_sand_offers_straight_down_into_air() {
    let mut w = world(5, 5, &[(2, 1)]);
    w.step();
    assert_eq!(w.cell(2, 1).offer, Some(Dir::Down));
}

#[test]
fn offer_sand_on_sand_offers_a_diagonal() {
    let mut w = world(5, 5, &[(2, 3), (2, 4)]);
    w.step();
    let offer = w.cell(2, 3).offer;
    assert!(matches!(offer, Some(Dir::DownLeft | Dir::DownRight)), "{offer:?}");
}

#[test]
fn offer_none_when_nothing_below_holds_air() {
    // An odd column's bottom cell is the lowest point: every downhill neighbor is outside.
    let mut w = world(5, 5, &[(3, 4)]);
    w.step();
    assert_eq!(w.cell(3, 4).offer, None, "the floor is solid");
}

#[test]
fn offer_even_bottom_cell_slides_to_the_lower_odd_column() {
    // Odd columns sit half a cell lower, so the floor is jagged and an even column's
    // bottom cell still has downhill neighbors.
    let mut w = world(5, 5, &[(2, 4)]);
    w.step();
    assert!(matches!(w.cell(2, 4).offer, Some(Dir::DownLeft | Dir::DownRight)));
}

#[test]
fn accept_air_accepts_the_offer_it_sees() {
    let mut w = world(5, 5, &[(2, 1)]);
    w.step(); // offer
    w.step(); // accept
    assert_eq!(w.cell(2, 2).accept, Some(Dir::Up));
}

/// What a cell sees: air all around, except the given neighbors.
fn seen(with: &[(Dir, State)]) -> [Seen; 6] {
    let mut out = [Seen::Cell(State::AIR); 6];
    for &(d, s) in with {
        out[d as usize] = Seen::Cell(s);
    }
    out
}

fn offering(d: Dir) -> State {
    State { material: Material::Sand, offer: Some(d), accept: None }
}

#[test]
fn accept_straight_above_beats_a_diagonal() {
    let around = seen(&[(Dir::Up, offering(Dir::Down)), (Dir::UpLeft, offering(Dir::DownRight))]);
    for phase in 0..32 {
        for x in [2, 3] {
            assert_eq!(next(State::AIR, around, x, 2, phase).accept, Some(Dir::Up));
        }
    }
}

#[test]
fn accept_diagonal_ties_are_hashed_and_pick_exactly_one() {
    let around = seen(&[(Dir::UpLeft, offering(Dir::DownRight)), (Dir::UpRight, offering(Dir::DownLeft))]);
    let picks: Vec<_> = (0..64).map(|phase| next(State::AIR, around, 2, 2, phase).accept).collect();
    assert!(picks.iter().all(|p| matches!(p, Some(Dir::UpLeft | Dir::UpRight))));
    assert!(picks.contains(&Some(Dir::UpLeft)) && picks.contains(&Some(Dir::UpRight)));
}

#[test]
fn accept_ignores_offers_aimed_elsewhere() {
    let around = seen(&[(Dir::UpLeft, offering(Dir::Down))]);
    assert_eq!(next(State::AIR, around, 2, 2, 0).accept, None);
}

#[test]
fn offer_withdrawn_when_the_target_accepts_another() {
    let me = offering(Dir::DownRight);
    let target = State { material: Material::Air, offer: None, accept: Some(Dir::Up) };
    // Everything else around is sand, so there is nowhere else to offer.
    let mut around = [Seen::Cell(State { material: Material::Sand, offer: None, accept: None }); 6];
    around[Dir::DownRight as usize] = Seen::Cell(target);
    assert_eq!(next(me, around, 2, 2, 0).offer, None);
}

#[test]
fn transfer_moves_one_grain_and_leaves_air() {
    let mut w = world(5, 5, &[(2, 1)]);
    w.step(); // offer
    w.step(); // accept
    w.step(); // transfer
    assert_eq!(sand_at(&w), vec![(2, 2)]);
    assert_eq!(w.cell(2, 1).material, Material::Air);
}

#[test]
fn transfer_falls_one_cell_every_two_phases_once_moving() {
    let mut w = world(3, 12, &[(1, 0)]);
    for _ in 0..3 {
        w.step();
    }
    assert_eq!(sand_at(&w), vec![(1, 1)]);
    for _ in 0..4 {
        w.step();
    }
    assert_eq!(sand_at(&w), vec![(1, 3)]);
}

#[test]
fn transfer_grain_comes_to_rest_on_the_floor() {
    let mut w = world(3, 6, &[(1, 0)]);
    for _ in 0..40 {
        w.step();
    }
    assert_eq!(sand_at(&w), vec![(1, 5)]);
}

// Conservation

#[test]
fn conservation_sand_count_never_changes() {
    let mut w = scattered(32, 32);
    let start = w.count(Material::Sand);
    assert!(start > 100);
    for _ in 0..200 {
        w.step();
        assert_eq!(w.count(Material::Sand), start);
    }
}

#[test]
fn conservation_no_transfer_duplicates_or_loses_under_contests() {
    // A dense block falling onto a floor is all contests.
    let mut w = World::new(16, 16);
    for y in 0..8 {
        for x in 4..12 {
            w.paint(x, y, Material::Sand);
        }
    }
    for _ in 0..300 {
        w.step();
        assert_eq!(w.count(Material::Sand), 64);
    }
}

// Piling

#[test]
fn piling_settles_with_no_air_downhill_of_any_grain() {
    let mut w = scattered(24, 24);
    for _ in 0..600 {
        w.step();
    }
    for (x, y) in sand_at(&w) {
        for d in Dir::ALL.into_iter().filter(|&d| drop(x, y, d) > 0) {
            let (nx, ny) = neighbor(x, y, d);
            if w.in_bounds(nx, ny) {
                assert_eq!(w.material(nx, ny), Material::Sand, "air below ({x},{y}) toward {d:?}");
            }
        }
    }
}

#[test]
fn piling_a_column_spreads_into_a_pile() {
    let mut w = World::new(31, 20);
    for y in 0..10 {
        w.paint(15, y, Material::Sand);
    }
    for _ in 0..600 {
        w.step();
    }
    let floor = (0..31).filter(|&x| w.material(x, 19) == Material::Sand).count();
    assert!(floor >= 3, "floor row holds {floor}");
    let column = (0..20).filter(|&y| w.material(15, y) == Material::Sand).count();
    assert!(column < 10, "column still {column} tall");
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
    let mut w = world(5, 5, &[(2, 1)]);
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
    let mut b = a.clone();
    a.step();
    b.update(5, 3);
    b.update(5, 4);
    b.update(6, 4);
    for y in 0..12 {
        for x in 0..12 {
            if b.phase(x, y) == 0 {
                assert!(b.update(x, y));
            }
        }
    }
    assert_eq!(a, b);
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
fn brush_erasing_a_grain_mid_handshake_creates_nothing() {
    let mut w = world(5, 5, &[(2, 1)]);
    w.step(); // offer
    w.step(); // accept
    w.paint(2, 1, Material::Air);
    for _ in 0..10 {
        w.step();
    }
    assert_eq!(w.count(Material::Sand), 0);
}

// Encoding

#[test]
fn encoding_state_round_trips() {
    for material in [Material::Air, Material::Sand] {
        for offer in [None, Some(Dir::Down), Some(Dir::UpLeft)] {
            for accept in [None, Some(Dir::Up), Some(Dir::DownRight)] {
                let s = State { material, offer, accept };
                assert_eq!(State::decode(s.encode()), s);
            }
        }
    }
}
