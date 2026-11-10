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
fn seen(with: &[(Dir, State)]) -> [State; 6] {
    let mut out = [State::AIR; 6];
    for &(d, s) in with {
        out[d as usize] = s;
    }
    out
}

fn offering(d: Dir) -> State {
    State { material: Material::Sand, offer: Some(d), ..State::AIR }
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
    let target = State { accept: Some(Dir::Up), ..State::AIR };
    // Everything else around is sand, so there is nowhere else to offer.
    let mut around = [State::holding(Material::Sand); 6];
    around[Dir::DownRight as usize] = target;
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
    // A cell holds at most one intent: sand and water may offer, air and water may accept, slate does
    // neither. Water has a flow; sand carrying a packet holds the packet's flow.
    let mut states = vec![State::holding(Material::Slate), State::holding(Material::Bedrock)];
    for d in [None, Some(Dir::Down), Some(Dir::UpLeft)] {
        states.push(State { material: Material::Sand, offer: d, ..State::AIR });
        states.push(State { accept: d, ..State::AIR });
        for flow in [Side::Left, Side::Right] {
            states.push(State { material: Material::Sand, offer: d, carries: true, flow, ..State::AIR });
            states.push(State { material: Material::Water, accept: d, flow, ..State::AIR });
            for waited in [false, true] {
                states.push(State { material: Material::Water, offer: d, flow, waited, ..State::AIR });
            }
        }
    }
    for s in states {
        assert_eq!(State::decode(s.encode()), s);
    }
}

#[test]
fn encoding_state_has_9_bits_and_phase_14() {
    assert!(STATE_MASK.count_ones() == 9 && PHASE_MASK == (1 << 14) - 1);
}

// Slate

const SLATE: State = State::SLATE;

fn slate_world(w: i32, h: i32, slate: &[(i32, i32)], sand: &[(i32, i32)]) -> World {
    let mut world = World::new(w, h);
    for &(x, y) in slate {
        world.paint(x, y, Material::Slate);
    }
    for &(x, y) in sand {
        world.paint(x, y, Material::Sand);
    }
    world
}

#[test]
fn slate_never_moves() {
    let slate = [(2, 1), (3, 1), (2, 2)];
    let mut w = slate_world(6, 8, &slate, &[]);
    for _ in 0..50 {
        w.step();
    }
    for y in 0..8 {
        for x in 0..6 {
            let expect = if slate.contains(&(x, y)) { Material::Slate } else { Material::Air };
            assert_eq!(w.material(x, y), expect, "({x},{y})");
        }
    }
}

#[test]
fn slate_never_offers_or_accepts() {
    let around = seen(&[(Dir::Up, offering(Dir::Down))]);
    for phase in 0..8 {
        assert_eq!(next(SLATE, around, 2, 2, phase), SLATE);
    }
}

#[test]
fn offer_sand_never_offers_to_slate() {
    let walled = seen(&[(Dir::Down, SLATE), (Dir::DownLeft, SLATE), (Dir::DownRight, SLATE)]);
    assert_eq!(next(State::holding(Material::Sand), walled, 2, 2, 0).offer, None);
    let gap = seen(&[(Dir::Down, SLATE), (Dir::DownRight, SLATE)]);
    assert_eq!(next(State::holding(Material::Sand), gap, 2, 2, 0).offer, Some(Dir::DownLeft));
}

#[test]
fn slate_holds_sand_up() {
    // A shelf of slate across the middle: sand dropped on it rests on it.
    let shelf: Vec<_> = (0..7).map(|x| (x, 4)).collect();
    let mut w = slate_world(7, 8, &shelf, &[(3, 0)]);
    for _ in 0..60 {
        w.step();
    }
    let sand = sand_at(&w);
    assert_eq!(sand.len(), 1);
    let (x, y) = sand[0];
    assert!(y < 4, "sand at ({x},{y}) went through the shelf");
    assert!(Dir::ALL.into_iter().filter(|&d| drop(x, y, d) > 0).all(|d| {
        let (nx, ny) = neighbor(x, y, d);
        w.material(nx, ny) == Material::Slate
    }));
}

#[test]
fn conservation_slate_and_sand_counts_never_change() {
    let mut w = scattered(24, 24);
    for x in 4..20 {
        w.paint(x, 16, Material::Slate);
    }
    let (sand, slate) = (w.count(Material::Sand), w.count(Material::Slate));
    for _ in 0..300 {
        w.step();
        assert_eq!((w.count(Material::Sand), w.count(Material::Slate)), (sand, slate));
    }
}

#[test]
fn brush_paints_only_into_air() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Sand);
    w.paint(1, 1, Material::Slate);
    assert_eq!(w.material(1, 1), Material::Sand, "slate does not replace sand");
    w.paint(2, 2, Material::Slate);
    w.paint(2, 2, Material::Sand);
    assert_eq!(w.material(2, 2), Material::Slate, "sand does not replace slate");
}

#[test]
fn brush_stamp_covers_a_disc_of_centres() {
    let mut w = World::new(9, 9);
    w.stamp(4.0, 4.0, 1.2, Material::Sand);
    // Even column 4: (4,3), (4,4), (4,5); odd columns 3 and 5 sit half a cell lower: rows 3 and 4.
    assert_eq!(sand_at(&w), vec![(3, 3), (4, 3), (5, 3), (3, 4), (4, 4), (5, 4), (4, 5)]);
}

#[test]
fn brush_erases_slate() {
    let mut w = slate_world(4, 4, &[(1, 1)], &[]);
    w.paint(1, 1, Material::Air);
    assert_eq!(w.count(Material::Slate), 0);
}

// Bedrock

#[test]
fn bedrock_is_the_boundary() {
    // A world with bedrock along its right side and floor evolves exactly like the bare world.
    // (Shifting the world to frame the left and top too would change the hash's tie-breaks;
    // every edge goes through the same out-of-bounds check.)
    let (w, h) = (10, 8);
    let mut open = World::new(w, h);
    for y in 0..h {
        for x in 0..w {
            if hash(x, y, 0, 13).is_multiple_of(2) {
                open.paint(x, y, Material::Sand);
            }
        }
    }
    let mut framed = World::new(w + 1, h + 1);
    for y in 0..h + 1 {
        for x in 0..w + 1 {
            let inside = x < w && y < h;
            framed.paint(x, y, if inside { open.material(x, y) } else { Material::Bedrock });
        }
    }
    for phase in 0..100 {
        open.step();
        framed.step();
        for y in 0..h {
            for x in 0..w {
                assert_eq!(open.cell(x, y), framed.cell(x, y), "({x},{y}) after {phase} phases");
            }
        }
    }
}

#[test]
fn bedrock_never_moves_offers_or_accepts() {
    let bedrock = State::holding(Material::Bedrock);
    let around = seen(&[(Dir::Up, offering(Dir::Down))]);
    assert_eq!(next(bedrock, around, 2, 2, 0), bedrock);
    let below = seen(&[(Dir::Down, bedrock), (Dir::DownLeft, bedrock), (Dir::DownRight, bedrock)]);
    assert_eq!(next(State::holding(Material::Sand), below, 2, 2, 0).offer, None);
}

#[test]
fn brush_cannot_erase_bedrock() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Bedrock);
    w.paint(1, 1, Material::Air);
    w.stamp(1.0, 1.0, 3.0, Material::Air);
    assert_eq!(w.material(1, 1), Material::Bedrock);
}

// Water

fn water_at(world: &World) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for y in 0..world.height() {
        for x in 0..world.width() {
            if world.material(x, y) == Material::Water {
                out.push((x, y));
            }
        }
    }
    out
}

fn water(flow: Side) -> State {
    State { material: Material::Water, flow, ..State::AIR }
}

fn water_offering(d: Dir, flow: Side) -> State {
    State { offer: Some(d), ..water(flow) }
}

/// Slate on the three downhill sides: nowhere to fall.
fn blocked_below(with: &[(Dir, State)]) -> [State; 6] {
    let mut around = seen(&[(Dir::Down, SLATE), (Dir::DownLeft, SLATE), (Dir::DownRight, SLATE)]);
    for &(d, s) in with {
        around[d as usize] = s;
    }
    around
}

#[test]
fn water_falls_like_sand() {
    let mut w = World::new(5, 5);
    w.paint(2, 1, Material::Water);
    w.step(); // offer
    w.step(); // accept
    w.step(); // transfer
    assert_eq!(water_at(&w), vec![(2, 2)]);
}

#[test]
fn water_falls_to_its_flow_side_when_both_diagonals_are_open() {
    for flow in [Side::Left, Side::Right] {
        let around = seen(&[(Dir::Down, SLATE)]);
        let want = if flow == Side::Left { Dir::DownLeft } else { Dir::DownRight };
        for phase in 0..16 {
            assert_eq!(next(water(flow), around, 2, 2, phase).offer, Some(want));
        }
    }
}

#[test]
fn side_water_that_cannot_fall_offers_to_its_flow_side() {
    let s = next(water(Side::Right), blocked_below(&[]), 2, 2, 0);
    assert_eq!((s.offer, s.flow, s.waited), (Some(Dir::UpRight), Side::Right, false));
    let s = next(water(Side::Left), blocked_below(&[]), 2, 2, 0);
    assert_eq!((s.offer, s.flow), (Some(Dir::UpLeft), Side::Left));
}

#[test]
fn side_water_takes_the_other_side_when_its_flow_side_is_shut() {
    let s = next(water(Side::Right), blocked_below(&[(Dir::UpRight, SLATE)]), 2, 2, 0);
    assert_eq!((s.offer, s.flow), (Some(Dir::UpLeft), Side::Left));
}

#[test]
fn side_sand_never_moves_sideways() {
    let s = next(State::holding(Material::Sand), blocked_below(&[]), 2, 2, 0);
    assert_eq!(s.offer, None);
}

#[test]
fn side_air_accepts_water_only_where_it_can_fall_on() {
    // The giver is down-left, offering up-right; the far lower diagonal is down-right.
    let giver = (Dir::DownLeft, water_offering(Dir::UpRight, Side::Right));
    for x in [2, 3] {
        let shut = seen(&[giver, (Dir::Down, SLATE), (Dir::DownRight, SLATE)]);
        assert_eq!(next(State::AIR, shut, x, 2, 0).accept, None, "level: declined");
        let far_open = seen(&[giver, (Dir::Down, SLATE)]);
        assert_eq!(next(State::AIR, far_open, x, 2, 0).accept, Some(Dir::DownLeft));
        let down_open = seen(&[giver, (Dir::DownRight, SLATE)]);
        assert_eq!(next(State::AIR, down_open, x, 2, 0).accept, Some(Dir::DownLeft));
    }
}

#[test]
fn side_offers_lose_to_falls() {
    let around = seen(&[
        (Dir::DownLeft, water_offering(Dir::UpRight, Side::Right)),
        (Dir::UpRight, water_offering(Dir::DownLeft, Side::Left)),
    ]);
    for phase in 0..32 {
        assert_eq!(next(State::AIR, around, 2, 2, phase).accept, Some(Dir::UpRight));
    }
}

#[test]
fn side_offer_waits_one_phase_then_is_declined_and_flow_flips() {
    let me = water_offering(Dir::UpRight, Side::Right);
    let s = next(me, blocked_below(&[]), 2, 2, 0);
    assert_eq!((s.offer, s.waited), (Some(Dir::UpRight), true), "not yet seen by the air");
    let s = next(s, blocked_below(&[]), 2, 2, 1);
    assert_eq!((s.offer, s.flow, s.waited), (Some(Dir::UpLeft), Side::Left, false), "declined");
}

#[test]
fn transfer_carries_water_and_its_flow() {
    // Received from straight above: the flow carries over.
    let me = State { accept: Some(Dir::Up), ..State::AIR };
    let s = next(me, seen(&[(Dir::Up, water_offering(Dir::Down, Side::Left))]), 2, 2, 0);
    assert_eq!((s.material, s.flow), (Material::Water, Side::Left));
    // Received from up-left, moving right: the flow becomes right.
    let me = State { accept: Some(Dir::UpLeft), ..State::AIR };
    let s = next(me, seen(&[(Dir::UpLeft, water_offering(Dir::DownRight, Side::Left))]), 2, 2, 0);
    assert_eq!((s.material, s.flow), (Material::Water, Side::Right));
}

/// A slate ramp climbing to the right from `(1, h - 1)` along up-right neighbors.
fn ramp(world: &mut World) {
    let (mut x, mut y) = (1, world.height() - 1);
    while world.in_bounds(x, y) {
        world.paint(x, y, Material::Slate);
        (x, y) = neighbor(x, y, Dir::UpRight);
    }
}

#[test]
fn water_never_climbs_a_staircase() {
    // Water in the corner at the foot of a ramp: every side step up the ramp would be a climb.
    let mut w = World::new(16, 12);
    ramp(&mut w);
    w.put(0, 11, water(Side::Right));
    for _ in 0..300 {
        w.step();
        assert_eq!(water_at(&w), vec![(0, 11)]);
    }
}

#[test]
fn water_runs_along_a_floor_in_a_line() {
    // A drop on the floor keeps its flow: it covers ground linearly, not as a random walk.
    let mut w = World::new(48, 4);
    w.put(1, 3, water(Side::Right));
    for _ in 0..120 {
        w.step();
    }
    let [(x, _)] = water_at(&w)[..] else { panic!("one drop") };
    assert!(x >= 25, "only reached column {x}");
}

#[test]
fn water_settles_level() {
    let (wd, ht) = (24, 10);
    let mut w = World::new(wd, ht);
    for y in 1..ht {
        for x in 0..7 {
            w.paint(x, y, Material::Water);
        }
    }
    for _ in 0..3000 {
        w.step();
    }
    // The top of each column's resting water (nothing but air below it is in flight), in half
    // cells. A level surface alternates by one; the few extra drops of a partial top layer, which
    // skate along it looking for a hole, add one more.
    let resting =
        |x: i32, y: i32| w.material(x, y) == Material::Water && (y + 1 == ht || w.material(x, y + 1) != Material::Air);
    let tops: Vec<i32> = (0..wd).map(|x| (0..ht).find(|&y| resting(x, y)).map_or(i32::MIN, |y| height(x, y))).collect();
    let (lo, hi) = (*tops.iter().min().unwrap(), *tops.iter().max().unwrap());
    assert!(lo > i32::MIN, "every column holds water: {tops:?}");
    assert!(hi - lo <= 2, "surface not level: {tops:?}");
}

#[test]
fn water_level_surface_is_still() {
    // A pool filled exactly level stays put: no surface churn.
    let mut w = World::new(12, 8);
    for y in 5..8 {
        for x in 0..12 {
            w.paint(x, y, Material::Water);
        }
    }
    let before = water_at(&w);
    for _ in 0..200 {
        w.step();
        assert_eq!(water_at(&w), before);
    }
}

// Displacement

/// A pool: water filling the bottom `depth` rows.
fn pool(w: i32, h: i32, depth: i32) -> World {
    let mut world = World::new(w, h);
    for y in h - depth..h {
        for x in 0..w {
            world.paint(x, y, Material::Water);
        }
    }
    world
}

#[test]
fn displace_sand_offers_down_into_still_water() {
    let around = seen(&[(Dir::Down, water(Side::Left))]);
    assert_eq!(next(State::holding(Material::Sand), around, 2, 2, 0).offer, Some(Dir::Down));
}

#[test]
fn displace_sand_never_offers_to_moving_water() {
    let around = seen(&[
        (Dir::Down, water_offering(Dir::DownLeft, Side::Left)),
        (Dir::DownLeft, SLATE),
        (Dir::DownRight, SLATE),
    ]);
    assert_eq!(next(State::holding(Material::Sand), around, 2, 2, 0).offer, None);
}

#[test]
fn displace_still_water_accepts_sand_from_above() {
    // Shut in but for the sand above: the water can't move, so it takes the grain's offer.
    let around = blocked_below(&[(Dir::Up, offering(Dir::Down)), (Dir::UpLeft, SLATE), (Dir::UpRight, SLATE)]);
    let s = next(water(Side::Right), around, 2, 2, 0);
    assert_eq!((s.accept, s.offer), (Some(Dir::Up), None));
}

#[test]
fn displace_water_that_can_move_does_not_accept() {
    let s = next(water(Side::Left), seen(&[(Dir::Up, offering(Dir::Down))]), 2, 2, 0);
    assert_eq!((s.accept, s.offer), (None, Some(Dir::Down)));
}

#[test]
fn displace_water_never_accepts_water() {
    let around = blocked_below(&[(Dir::Up, water_offering(Dir::Down, Side::Left))]);
    assert_eq!(next(water(Side::Left), around, 2, 2, 0).accept, None);
}

#[test]
fn displace_is_two_transfers_over_successive_phases() {
    // A grain on a level pool one row deep: it offers, the water accepts, the grain moves in and
    // the water it displaced waits in the grain's cell as a packet, which goes up the next phase.
    let mut w = pool(5, 5, 1);
    w.paint(2, 3, Material::Sand);
    w.step(); // offer
    assert_eq!(w.cell(2, 3).offer, Some(Dir::Down), "{}", w.ascii_intents());
    w.step(); // accept
    assert_eq!(w.cell(2, 4).accept, Some(Dir::Up), "{}", w.ascii_intents());
    w.step(); // the grain moves down
    let (gap, below) = (w.cell(2, 3), w.cell(2, 4));
    assert_eq!((gap.material, gap.accept), (Material::Air, Some(Dir::Down)), "the gap waits for the water");
    assert_eq!((below.material, below.carries, below.offer), (Material::Sand, true, Some(Dir::Up)));
    assert_eq!((w.count(Material::Sand), w.count(Material::Water)), (1, 5), "the packet counts as water");
    w.step(); // the water moves up
    assert_eq!(w.material(2, 3), Material::Water, "{}", w.ascii_intents());
    assert_eq!((w.material(2, 4), w.cell(2, 4).carries), (Material::Sand, false));
    assert_eq!((w.count(Material::Sand), w.count(Material::Water)), (1, 5));
}

#[test]
fn displace_sand_sinks_to_the_floor_of_a_pool() {
    let mut w = pool(9, 14, 10);
    w.paint(4, 1, Material::Sand);
    for _ in 0..200 {
        w.step();
        assert_eq!((w.count(Material::Sand), w.count(Material::Water)), (1, 90));
    }
    let [(_, y)] = sand_at(&w)[..] else { panic!("one grain") };
    assert_eq!(y, 13, "{}", w.ascii());
}

#[test]
fn displace_sand_sinks_slower_than_it_falls() {
    let reach_floor = |mut w: World| {
        (1..).find(|_| {
            w.step();
            sand_at(&w).first().is_some_and(|&(_, y)| y == 19)
        })
    };
    let (mut dry, wet) = (World::new(5, 20), pool(5, 20, 18));
    dry.paint(2, 1, Material::Sand);
    let mut wet = wet;
    wet.paint(2, 1, Material::Sand);
    let (dry, wet) = (reach_floor(dry).unwrap(), reach_floor(wet).unwrap());
    assert!(wet > dry + 10, "dry {dry}, wet {wet}");
}

#[test]
fn displace_sand_piles_under_water() {
    let mut w = pool(20, 20, 12);
    for y in 1..6 {
        for x in 8..12 {
            w.paint(x, y, Material::Sand);
        }
    }
    let counts = (w.count(Material::Sand), w.count(Material::Water));
    for _ in 0..1500 {
        w.step();
        assert_eq!((w.count(Material::Sand), w.count(Material::Water)), counts);
    }
    // Settled: no grain has a lighter neighbor downhill, and no packet is left in flight.
    for (x, y) in sand_at(&w) {
        assert!(!w.cell(x, y).carries, "a packet left at ({x},{y})\n{}", w.ascii());
        for d in [Dir::Down, Dir::DownLeft, Dir::DownRight] {
            let (nx, ny) = neighbor(x, y, d);
            if w.in_bounds(nx, ny) {
                assert!(
                    !matches!(w.material(nx, ny), Material::Air | Material::Water),
                    "grain at ({x},{y}) could still sink {d:?}\n{}",
                    w.ascii()
                );
            }
        }
    }
}

#[test]
fn displace_water_never_sinks_into_sand() {
    let mut w = World::new(10, 10);
    for y in 6..10 {
        for x in 0..10 {
            w.paint(x, y, Material::Sand);
        }
    }
    for x in 0..10 {
        w.paint(x, 5, Material::Water);
    }
    let sand = sand_at(&w);
    for _ in 0..200 {
        w.step();
        assert_eq!(sand_at(&w), sand);
    }
}

#[test]
fn displace_brush_leaves_the_gap_alone() {
    // A grain sinking through deep water: its gap has no air around it, so if the brush could fill
    // it, the packet would have nowhere to go and the grain would hang mid-pool.
    for m in [Material::Water, Material::Sand, Material::Slate, Material::Air] {
        let mut w = World::new(5, 6);
        for y in 1..6 {
            for x in 0..5 {
                w.paint(x, y, Material::Water);
            }
        }
        w.paint(2, 0, Material::Sand);
        for _ in 0..6 {
            w.step();
        }
        assert_eq!((w.material(2, 1), w.cell(2, 1).accept), (Material::Air, Some(Dir::Down)), "{m:?}");
        w.paint(2, 1, m);
        assert_eq!(w.material(2, 1), Material::Air, "{m:?}: the brush leaves the gap alone");
        for _ in 0..100 {
            w.step();
            assert_eq!(w.count(Material::Water), 25);
        }
        let [(x, y)] = sand_at(&w)[..] else { panic!("one grain") };
        assert!(y == 5 && !w.cell(x, y).carries, "{m:?}: the grain reaches the floor\n{}", w.ascii_intents());
    }
}

#[test]
fn displace_settles_under_a_busy_brush() {
    for m in [Material::Water, Material::Sand, Material::Slate, Material::Air] {
        let mut w = pool(20, 24, 18);
        for y in 1..5 {
            for x in 7..13 {
                w.paint(x, y, Material::Sand);
            }
        }
        for phase in 0..3000 {
            if (40..200).contains(&phase) && phase % 3 == 0 {
                w.stamp(10.0, 12.0, 8.0, m);
            }
            w.step();
        }
        for (x, y) in sand_at(&w) {
            assert!(!w.cell(x, y).carries, "{m:?}: a packet stranded at ({x},{y})\n{}", w.ascii());
        }
    }
}

#[test]
fn synchrony_displacement_is_independent_of_evaluation_order() {
    let mut a = pool(20, 20, 10);
    for y in 0..9 {
        for x in 0..20 {
            if hash(x, y, 0, 9).is_multiple_of(2) {
                a.paint(x, y, Material::Sand);
            }
        }
    }
    let mut b = a.clone();
    let cells: Vec<_> = (0..20).flat_map(|y| (0..20).map(move |x| (x, y))).collect();
    for phase in 0..200 {
        a.step();
        let mut order = cells.clone();
        order.sort_by_key(|&(x, y)| hash(x, y, phase, 10));
        for (x, y) in order {
            assert!(b.update(x, y));
        }
        assert_eq!(a, b, "phase {phase}");
    }
}

#[test]
fn conservation_water_and_sand_counts_never_change() {
    let mut w = World::new(32, 32);
    for y in 0..20 {
        for x in 0..32 {
            match hash(x, y, 0, 7) % 4 {
                0 => w.paint(x, y, Material::Sand),
                1 => w.paint(x, y, Material::Water),
                _ => {}
            }
        }
    }
    let (sand, water) = (w.count(Material::Sand), w.count(Material::Water));
    assert!(sand > 100 && water > 100);
    for _ in 0..400 {
        w.step();
        assert_eq!((w.count(Material::Sand), w.count(Material::Water)), (sand, water));
    }
}

#[test]
fn synchrony_water_is_independent_of_evaluation_order() {
    let mut a = World::new(20, 20);
    for y in 0..14 {
        for x in 0..20 {
            if hash(x, y, 0, 8).is_multiple_of(2) {
                a.paint(x, y, if x < 8 { Material::Sand } else { Material::Water });
            }
        }
    }
    let mut b = a.clone();
    let cells: Vec<_> = (0..20).flat_map(|y| (0..20).map(move |x| (x, y))).collect();
    for phase in 0..120 {
        a.step();
        let mut order = cells.clone();
        order.sort_by_key(|&(x, y)| hash(x, y, phase, 6));
        for (x, y) in order {
            assert!(b.update(x, y));
        }
        assert_eq!(a, b, "phase {phase}");
    }
}

#[test]
fn brush_paints_water_only_into_air_and_erases_it() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Sand);
    w.paint(1, 1, Material::Water);
    assert_eq!(w.material(1, 1), Material::Sand);
    w.paint(2, 2, Material::Water);
    assert_eq!(w.material(2, 2), Material::Water);
    w.paint(2, 2, Material::Sand);
    assert_eq!(w.material(2, 2), Material::Water, "sand does not replace water");
    w.paint(2, 2, Material::Air);
    assert_eq!(w.count(Material::Water), 0);
}

#[test]
fn brush_gives_painted_water_a_hashed_flow() {
    let mut w = World::new(16, 4);
    for x in 0..16 {
        w.paint(x, 1, Material::Water);
    }
    let flows: Vec<_> = (0..16).map(|x| w.cell(x, 1).flow).collect();
    assert!(flows.contains(&Side::Left) && flows.contains(&Side::Right));
}
