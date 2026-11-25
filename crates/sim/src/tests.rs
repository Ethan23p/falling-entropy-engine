//! Tests, named after what they pin: the update's stages (`propose_*`, `evaluate_*`, `resolve_*`),
//! the physics distribution's policies (`movement_*`, `gravity_*`), and the invariants (geometry,
//! randomness, synchrony, conservation, the brush, the encoding).

use crate::update::next;
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
fn conservation_the_census_never_changes() {
    // Sand overwrites air and its origin is granted an air, so both counts hold, measured on
    // published state with each particle in transport counted once.
    let mut w = scattered(20, 16);
    let counts = (w.count(Material::Sand), w.count(Material::Air));
    for _ in 0..300 {
        w.step();
        assert_eq!((w.count(Material::Sand), w.count(Material::Air)), counts);
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
fn brush_leaves_cells_engaged_in_a_contract_alone() {
    let mut w = World::new(3, 4);
    w.paint(1, 0, Material::Sand);
    w.physics.restless = 1;
    w.step(); // the grain offers
    let (x, y) = neighbor(1, 0, w.cell(1, 0).offer.expect("an offer"));
    w.paint(1, 0, Material::Air);
    assert_eq!(w.material(1, 0), Material::Sand, "an offer out");
    w.step(); // accepted
    w.paint(x, y, Material::Air);
    w.paint(1, 0, Material::Air);
    assert_eq!((w.material(x, y), w.material(1, 0)), (Material::Sand, Material::Sand), "in transport");
    w.step(); // resolved: the origin holds a granted air and nothing is pending there
    w.paint(1, 0, Material::Sand);
    assert_eq!((w.material(1, 0), w.count(Material::Sand)), (Material::Sand, 2));
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
    let velocities = [Velocity::ZERO, Velocity { x: -300, y: 32767 }, Velocity { x: i16::MIN, y: -1 }];
    for material in [Material::Air, Material::Sand, Material::Bedrock] {
        for velocity in velocities {
            for offer in [None, Some(Dir::Up), Some(Dir::UpLeft)] {
                for accept in [None, Some(Dir::Down), Some(Dir::UpRight)] {
                    let s = State { material, velocity, offer, accept };
                    assert_eq!(State::decode(s.encode(), s.velocity.encode()), s);
                    assert!(s.encode() < 1 << 19, "19 bits");
                }
            }
        }
    }
}

#[test]
fn encoding_a_cell_keeps_words_per_phase_parity() {
    // The state of phase p goes in word p % 2, tagged with p, and its velocity in word 2 + p % 2,
    // so the words a neighbor at p reads stay put until p + 2.
    let mut w = World::new(3, 3);
    w.paint(1, 0, Material::Bedrock);
    const { assert!(STATE_MASK == (1 << 19) - 1 && PHASE_MASK == (1 << 13) - 1 && WORDS == 4) };
    w.step();
    let words = &w.words()[WORDS..2 * WORDS];
    assert_eq!((words[0] >> 19, words[1] >> 19), (0, 1));
    assert_eq!(State::decode(words[0] & STATE_MASK, words[2]), State::BEDROCK);
    assert_eq!((w.kept(1, 0), w.cell(1, 0)), (State::BEDROCK, State::BEDROCK));
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

// The update's stages, one cell at a time

const FALLING: Velocity = Velocity { x: 0, y: 2048 };

fn sand(velocity: Velocity) -> State {
    State { velocity, ..State::holding(Material::Sand) }
}

fn offering(d: Dir, velocity: Velocity) -> State {
    State { offer: Some(d), ..sand(velocity) }
}

/// What a cell sees: air all around, except the given neighbors.
fn around(with: &[(Dir, State)]) -> [State; 6] {
    let mut out = [State::AIR; 6];
    for &(d, s) in with {
        out[d as usize] = s;
    }
    out
}

/// How often, over many phases, a cell's update proposes toward each direction (and nowhere, last).
fn proposals(me: State, seen: [State; 6], physics: &Physics) -> [u32; 7] {
    let mut out = [0; 7];
    for phase in 0..4000 {
        let s = next(me, State::AIR, seen, 5, 5, phase, physics);
        out[s.offer.map_or(6, |d| d as usize)] += 1;
    }
    out
}

#[test]
fn propose_a_particle_at_rest_has_no_lean() {
    // Zero velocity: every neighbor holding air is equally likely.
    let counts = proposals(sand(Velocity::ZERO), around(&[]), &Physics::default());
    for &c in &counts[..6] {
        assert!((550..790).contains(&c), "{counts:?}");
    }
    assert_eq!(counts[6], 0, "a compatible neighbor is always proposed to");
}

#[test]
fn propose_follows_velocity() {
    let counts = proposals(sand(FALLING), around(&[]), &Physics::default());
    let down = counts[Dir::Down as usize];
    let diagonals = counts[Dir::DownLeft as usize] + counts[Dir::DownRight as usize];
    let upward = counts[Dir::Up as usize] + counts[Dir::UpLeft as usize] + counts[Dir::UpRight as usize];
    assert!(down > diagonals / 2 && diagonals > 4 * upward, "{counts:?}");
    assert!(upward > 0, "backwards is unlikely, not excluded: {counts:?}");
}

#[test]
fn propose_excludes_bedrock_and_prefers_air_to_sand() {
    let seen = around(&[(Dir::Down, State::BEDROCK), (Dir::DownLeft, sand(Velocity::ZERO))]);
    let counts = proposals(sand(FALLING), seen, &Physics::default());
    assert_eq!(counts[Dir::Down as usize], 0, "{counts:?}");
    assert!(counts[Dir::DownRight as usize] > 8 * counts[Dir::DownLeft as usize], "{counts:?}");
    assert!(counts[Dir::DownLeft as usize] > 0, "sand into sand is unlikely, not excluded: {counts:?}");
}

#[test]
fn propose_nothing_where_nothing_is_compatible() {
    let walled = [State::BEDROCK; 6];
    assert_eq!(proposals(sand(FALLING), walled, &Physics::default())[6], 4000);
    // Air and bedrock offer nowhere.
    assert_eq!(proposals(State::AIR, around(&[]), &Physics::default())[6], 4000);
    assert_eq!(proposals(State::BEDROCK, around(&[]), &Physics::default())[6], 4000);
}

#[test]
fn propose_sand_into_air_toggled_off() {
    let off = Physics { sand_into_air: false, ..Physics::default() };
    let counts = proposals(sand(FALLING), around(&[(Dir::Down, sand(Velocity::ZERO))]), &off);
    assert_eq!(counts[Dir::Down as usize] + counts[6], 4000, "only the sand below: {counts:?}");
}

#[test]
fn propose_a_committed_particle_proposes_nothing_new() {
    // My offer is out, being evaluated: I neither offer again nor accept an offer into me.
    let me = offering(Dir::Down, FALLING);
    let seen = around(&[(Dir::Up, offering(Dir::Down, FALLING))]);
    for phase in 0..50 {
        let s = next(me, State::AIR, seen, 2, 2, phase, &Physics::default());
        assert_eq!(s, sand(FALLING), "the offer is cleared from the published state; the particle stays");
    }
}

#[test]
fn evaluate_air_accepts_one_offer_and_is_overwritten() {
    let seen = around(&[
        (Dir::Up, offering(Dir::Down, FALLING)),
        (Dir::UpLeft, offering(Dir::DownRight, FALLING)),
        (Dir::UpRight, offering(Dir::DownLeft, FALLING)),
    ]);
    let mut accepted = [0; 6];
    for phase in 0..3000 {
        let s = next(State::AIR, State::AIR, seen, 2, 2, phase, &Physics::default());
        assert_eq!(s.material, Material::Sand);
        accepted[s.accept.expect("one accepted") as usize] += 1;
    }
    // Biased by the offering particles' velocity: straight down is the steadiest.
    assert!(accepted[Dir::Up as usize] > accepted[Dir::UpLeft as usize], "{accepted:?}");
    assert!(accepted[Dir::UpLeft as usize] > 0 && accepted[Dir::UpRight as usize] > 0, "{accepted:?}");
}

#[test]
fn evaluate_sand_rejects_every_offer() {
    let seen = around(&[(Dir::Up, offering(Dir::Down, FALLING))]);
    let s = next(sand(Velocity::ZERO), State::AIR, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.accept), (Material::Sand, None));
}

#[test]
fn evaluate_rejects_into_air_when_toggled_off() {
    let off = Physics { sand_into_air: false, ..Physics::default() };
    let seen = around(&[(Dir::Up, offering(Dir::Down, FALLING))]);
    let s = next(State::AIR, State::AIR, seen, 2, 2, 0, &off);
    assert_eq!(s, State::AIR);
}

#[test]
fn gravity_is_an_impulse_on_each_move_scaled_by_mass() {
    let p = Physics::default();
    let v = Velocity { x: 100, y: -50 };
    let s = next(State::AIR, State::AIR, around(&[(Dir::UpLeft, offering(Dir::DownRight, v))]), 2, 2, 0, &p);
    let impulse = (p.gravity * p.mass_sand) as i16;
    assert_eq!(s.velocity, Velocity { x: 100, y: -50 + impulse });
}

#[test]
fn gravity_saturates() {
    let p = Physics::default();
    assert_eq!(p.moved(Material::Sand, Velocity { x: 0, y: 32700 }).y, i16::MAX);
}

#[test]
fn resolve_accepted_the_origin_is_granted_an_air() {
    let kept = offering(Dir::Down, FALLING);
    let seen = around(&[(Dir::Down, State { accept: Some(Dir::Up), ..sand(FALLING) })]);
    let s = next(sand(FALLING), kept, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.velocity, s.offer), (Material::Air, Velocity::ZERO, None));
}

#[test]
fn resolve_rejected_the_origin_keeps_its_particle_and_velocity() {
    // The neighbor below accepted someone else; mine was rejected. I propose again at once.
    let kept = offering(Dir::Down, FALLING);
    let seen = around(&[(Dir::Down, State { accept: Some(Dir::UpLeft), ..sand(FALLING) })]);
    let s = next(sand(FALLING), kept, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.velocity), (Material::Sand, FALLING));
    assert!(s.offer.is_some());
}

// Contracts across the lattice

fn world(w: i32, h: i32, sand_at: &[(i32, i32)]) -> World {
    let mut world = World::new(w, h);
    for &(x, y) in sand_at {
        world.paint(x, y, Material::Sand);
    }
    world
}

#[test]
fn contract_offer_evaluate_resolve_takes_three_phases() {
    let mut w = world(5, 5, &[(2, 0)]);
    // A wall of bedrock everywhere but below, so the grain's one compatible neighbor is (2, 1).
    for (x, y) in [(1, 0), (3, 0), (1, 1), (3, 1)] {
        w.paint(x, y, Material::Bedrock);
    }
    w.step(); // propose
    assert_eq!(w.cell(2, 0).offer, Some(Dir::Down), "{}", w.ascii());
    w.step(); // evaluate
    assert_eq!((w.cell(2, 1).material, w.cell(2, 1).accept), (Material::Sand, Some(Dir::Up)));
    assert_eq!(w.material(2, 0), Material::Sand, "the origin still holds it until it resolves");
    assert_eq!(w.count(Material::Sand), 1);
    w.step(); // resolve
    assert_eq!(w.material(2, 0), Material::Air);
    assert_eq!(w.cell(2, 1).velocity, Physics::default().moved(Material::Sand, Velocity::ZERO));
}

#[test]
fn contract_a_receiver_proposes_onward_in_the_same_update() {
    let mut w = world(3, 6, &[(1, 0)]);
    w.paint(0, 0, Material::Bedrock);
    w.paint(2, 0, Material::Bedrock);
    w.step();
    w.step();
    let s = w.cell(1, 1);
    assert!(s.accept.is_some() && s.offer.is_some(), "{s:?}");
}

// Movement, on whole lattices

/// Mean row of the given material: higher is lower down.
fn mean_row(w: &World, m: Material) -> f64 {
    let rows: Vec<i32> = (0..w.height())
        .flat_map(|y| (0..w.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| w.material(x, y) == m)
        .map(|(_, y)| y)
        .collect();
    rows.iter().sum::<i32>() as f64 / rows.len() as f64
}

fn packed(w: i32, h: i32) -> World {
    let mut world = World::new(w, h);
    for y in 0..h {
        for x in 0..w {
            if hash(x, y, 0, 41) & 1 == 1 {
                world.paint(x, y, Material::Sand);
            }
        }
    }
    world
}

#[test]
fn movement_a_packed_lattice_sorts_air_above_sand() {
    let mut w = packed(16, 16);
    let before = mean_row(&w, Material::Sand) - mean_row(&w, Material::Air);
    for _ in 0..600 {
        w.step();
    }
    let after = mean_row(&w, Material::Sand) - mean_row(&w, Material::Air);
    assert!(after > before + 4.0, "sand {before:.1} rows below air, then {after:.1}\n{}", w.ascii());
}

#[test]
fn movement_a_packed_lattice_is_still_with_sand_into_air_off() {
    let mut w = packed(12, 12);
    w.physics.sand_into_air = false;
    let start: Vec<Material> = (0..144).map(|i| w.material(i % 12, i / 12)).collect();
    for _ in 0..200 {
        w.step();
        let now: Vec<Material> = (0..144).map(|i| w.material(i % 12, i / 12)).collect();
        assert_eq!(now, start, "{}", w.ascii());
    }
}

#[test]
fn movement_a_grain_reaches_the_floor() {
    let mut w = world(9, 20, &[(4, 0)]);
    for _ in 0..400 {
        w.step();
    }
    let (x, y) =
        (0..20).flat_map(|y| (0..9).map(move |x| (x, y))).find(|&(x, y)| w.material(x, y) == Material::Sand).unwrap();
    assert!(y >= 18, "({x}, {y})\n{}", w.ascii());
}
