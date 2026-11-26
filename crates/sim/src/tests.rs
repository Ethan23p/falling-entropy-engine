//! Tests, named after what they pin: the update's stages (`propose_*`, `evaluate_*`, `resolve_*`),
//! the physics distribution's policies (`movement_*`, `gravity_*`), and the invariants (geometry,
//! randomness, synchrony, conservation, the brush, the encoding).

use crate::contract::{Effect, Offer, Verdict};
use crate::update::next;
use crate::world::STATE_MASK;
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
    // Contracts never create or lose a substantial particle. Every count holds, measured on published
    // state with each particle in transport counted once. (Air is insubstantial and not conserved.)
    let mut w = scattered(20, 16);
    for i in 0..12 {
        w.paint(3 + i, 4, Material::Air);
        w.paint(3 + i, 10, Material::Water);
        w.paint(3 + i, 14, Material::Water);
    }
    w.paint(10, 13, Material::Slate);
    let census = w.conserved();
    for _ in 0..300 {
        w.step();
        assert_eq!(w.conserved(), census, "{}", w.ascii());
    }
}

// Brush

#[test]
fn brush_paints_and_erases() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Sand);
    assert_eq!(w.count(Material::Sand), 1);
    w.paint(1, 1, Material::Vacuum);
    assert_eq!(w.count(Material::Sand), 0);
}

#[test]
fn brush_paints_only_into_vacuum() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Sand);
    w.paint(1, 1, Material::Bedrock);
    assert_eq!(w.material(1, 1), Material::Sand, "bedrock does not replace sand");
    w.paint(2, 2, Material::Bedrock);
    w.paint(2, 2, Material::Sand);
    assert_eq!(w.material(2, 2), Material::Bedrock, "sand does not replace bedrock");
    w.paint(3, 3, Material::Air);
    w.paint(3, 3, Material::Sand);
    assert_eq!(w.material(3, 3), Material::Air, "sand does not replace air");
}

#[test]
fn brush_paints_slate_and_erases_it() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Slate);
    w.paint(1, 1, Material::Sand);
    w.paint(1, 1, Material::Water);
    assert_eq!(w.material(1, 1), Material::Slate, "neither sand nor water replaces slate");
    w.paint(1, 1, Material::Vacuum);
    assert_eq!(w.material(1, 1), Material::Vacuum, "unlike bedrock, slate erases");
}

#[test]
fn brush_cannot_erase_bedrock() {
    let mut w = World::new(4, 4);
    w.paint(1, 1, Material::Bedrock);
    w.paint(1, 1, Material::Vacuum);
    w.stamp(1.0, 1.0, 3.0, Material::Vacuum);
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
    w.step(); // the grain offers
    let (x, y) = neighbor(1, 0, w.cell(1, 0).offer.expect("an offer"));
    w.paint(1, 0, Material::Vacuum);
    assert_eq!(w.material(1, 0), Material::Sand, "an offer out");
    w.step(); // accepted
    w.paint(x, y, Material::Vacuum);
    w.paint(1, 0, Material::Vacuum);
    assert_eq!((w.material(x, y), w.material(1, 0)), (Material::Sand, Material::Sand), "in transport");
    w.step(); // resolved: the origin is left vacuum and nothing is pending there
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
    for material in Material::ALL {
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

const DOWN: Velocity = Velocity::DOWN;

fn sand(velocity: Velocity) -> State {
    State { velocity, ..State::holding(Material::Sand) }
}

fn offering(d: Dir, velocity: Velocity) -> State {
    State { offer: Some(d), ..sand(velocity) }
}

/// What a cell sees: vacuum all around, except the given neighbors.
fn around(with: &[(Dir, State)]) -> [State; 6] {
    let mut out = [State::VACUUM; 6];
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
fn propose_a_new_particle_intends_to_move_down() {
    assert_eq!((State::AIR.velocity, State::holding(Material::Sand).velocity), (DOWN, DOWN));
    assert_eq!((State::VACUUM.velocity, State::BEDROCK.velocity), (Velocity::ZERO, Velocity::ZERO));
}

#[test]
fn propose_follows_velocity() {
    // Straight down is favored; the lower diagonals less; never against the velocity.
    let counts = proposals(sand(DOWN), around(&[]), &Physics::default());
    let down = counts[Dir::Down as usize];
    let diagonals = counts[Dir::DownLeft as usize] + counts[Dir::DownRight as usize];
    let upward = counts[Dir::Up as usize] + counts[Dir::UpLeft as usize] + counts[Dir::UpRight as usize];
    assert!(down > diagonals && diagonals > 0, "{counts:?}");
    assert_eq!(upward + counts[6], 0, "{counts:?}");
    // A sideways velocity leans the other way.
    let counts = proposals(sand(Velocity { x: 256, y: 0 }), around(&[]), &Physics::default());
    assert!(counts[Dir::UpRight as usize] > 0 && counts[Dir::DownRight as usize] > 0, "{counts:?}");
    assert_eq!(counts[Dir::Up as usize] + counts[Dir::Down as usize] + counts[Dir::UpLeft as usize], 0);
}

#[test]
fn propose_excludes_bedrock_and_prefers_air_to_sand() {
    let seen = around(&[(Dir::Down, State::BEDROCK), (Dir::DownLeft, sand(DOWN)), (Dir::DownRight, State::AIR)]);
    let counts = proposals(sand(DOWN), seen, &Physics::default());
    assert_eq!(counts[Dir::Down as usize], 0, "{counts:?}");
    assert!(counts[Dir::DownRight as usize] > 8 * counts[Dir::DownLeft as usize], "{counts:?}");
    assert!(counts[Dir::DownLeft as usize] > 0, "sand into sand is unlikely, not excluded: {counts:?}");
}

#[test]
fn propose_nothing_where_nothing_is_compatible() {
    let walled = [State::BEDROCK; 6];
    assert_eq!(proposals(sand(DOWN), walled, &Physics::default())[6], 4000);
    // Nothing offers into slate; bedrock and vacuum offer nowhere.
    assert_eq!(proposals(State::AIR, [State::holding(Material::Slate); 6], &Physics::default())[6], 4000);
    assert_eq!(proposals(State::BEDROCK, [State::VACUUM; 6], &Physics::default())[6], 4000);
    assert_eq!(proposals(State::VACUUM, [State::VACUUM; 6], &Physics::default())[6], 4000);
}

#[test]
fn propose_air_and_sand_offer_into_vacuum() {
    let mut below = [State::BEDROCK; 6];
    below[Dir::Down as usize] = State::VACUUM;
    assert_eq!(proposals(State::AIR, below, &Physics::default())[Dir::Down as usize], 4000);
    assert_eq!(proposals(sand(DOWN), below, &Physics::default())[Dir::Down as usize], 4000);
}

#[test]
fn propose_into_other_particles_weighted_air_then_water_then_sand() {
    // Below: air to one side, water straight down, sand to the other: all may be offered into, by
    // their weight multipliers (and how well each lines up with the velocity).
    let p = Physics::default();
    let seen = [State::BEDROCK, State::BEDROCK, State::AIR, sand(DOWN), State::holding(Material::Sand), State::BEDROCK];
    let n = proposals(sand(DOWN), seen, &p);
    assert!(n[Dir::DownRight as usize] > 8 * n[Dir::DownLeft as usize] && n[Dir::DownLeft as usize] > 0, "{n:?}");
    let seen =
        [State::BEDROCK, State::BEDROCK, State::AIR, State::BEDROCK, State::holding(Material::Water), State::BEDROCK];
    let n = proposals(sand(DOWN), seen, &p);
    assert!(n[Dir::DownRight as usize] > n[Dir::DownLeft as usize] && n[Dir::DownLeft as usize] > 0, "{n:?}");
}

#[test]
fn propose_a_committed_particle_proposes_nothing_new() {
    // My offer is out, being evaluated: I neither offer again nor accept an offer into me.
    let me = offering(Dir::Down, DOWN);
    let seen = around(&[(Dir::Up, offering(Dir::Down, DOWN))]);
    for phase in 0..50 {
        let s = next(me, State::AIR, seen, 2, 2, phase, &Physics::default());
        assert_eq!(s, sand(DOWN), "the offer is cleared from the published state; the particle stays");
    }
}

#[test]
fn evaluate_vacuum_accepts_one_offer() {
    let seen = around(&[
        (Dir::Up, offering(Dir::Down, DOWN)),
        (Dir::UpLeft, offering(Dir::DownRight, DOWN)),
        (Dir::UpRight, offering(Dir::DownLeft, DOWN)),
    ]);
    let mut accepted = [0; 6];
    for phase in 0..3000 {
        let s = next(State::VACUUM, State::AIR, seen, 2, 2, phase, &Physics::default());
        assert_eq!(s.material, Material::Sand);
        accepted[s.accept.expect("one accepted") as usize] += 1;
    }
    // Biased by the offering particles' velocity: straight down is the steadiest.
    assert!(accepted[Dir::Up as usize] > accepted[Dir::UpLeft as usize], "{accepted:?}");
    assert!(accepted[Dir::UpLeft as usize] > 0 && accepted[Dir::UpRight as usize] > 0, "{accepted:?}");
}

#[test]
fn evaluate_vacuum_is_filled_and_favors_the_heavier() {
    let air_offering = State { offer: Some(Dir::UpRight), ..State::AIR };
    let seen = [
        State::VACUUM,
        State::VACUUM,
        State::VACUUM,
        State::VACUUM,
        offering(Dir::UpRight, DOWN), // sand below-left, offering up into me: against its velocity
        air_offering,
    ];
    // Neither offer points along its velocity, so both weigh the minimum; still one is accepted.
    let s = next(State::VACUUM, State::AIR, seen, 2, 2, 0, &Physics::default());
    assert!(s.accept.is_some(), "{s:?}");
    // From above, sand outweighs air.
    let seen = around(&[
        (Dir::Up, offering(Dir::Down, DOWN)),
        (Dir::UpLeft, State { offer: Some(Dir::DownRight), ..State::AIR }),
    ]);
    let mut sand_won = 0;
    for phase in 0..2000 {
        let s = next(State::VACUUM, State::AIR, seen, 2, 2, phase, &Physics::default());
        sand_won += (s.material == Material::Sand) as u32;
    }
    assert!(sand_won > 1800, "{sand_won}");
}

#[test]
fn evaluate_sand_rejects_every_offer() {
    let seen = around(&[(Dir::Up, offering(Dir::Down, DOWN))]);
    let s = next(sand(DOWN), State::AIR, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.accept), (Material::Sand, None));
}

#[test]
fn evaluate_water_rejects_every_offer() {
    // A cell holding a substantial particle has no capacity for another.
    let seen = around(&[(Dir::Up, offering(Dir::Down, DOWN))]);
    let s = next(State::holding(Material::Water), State::AIR, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.accept), (Material::Water, None));
}

#[test]
fn evaluate_air_gives_way_to_sand_but_not_to_air() {
    // Air is insubstantial: sand transfers in and the air ceases to exist. Air can't take air.
    let seen = around(&[(Dir::Up, offering(Dir::Down, DOWN))]);
    let s = next(State::AIR, State::AIR, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.accept), (Material::Sand, Some(Dir::Up)));
    let seen = around(&[(Dir::Up, State { offer: Some(Dir::Down), ..State::AIR })]);
    let s = next(State::AIR, State::AIR, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.accept), (Material::Air, None));
}

#[test]
fn evaluate_air_with_its_offer_out_still_gives_way_to_sand() {
    // Capacity alone decides what evaluate entertains: committed air is still insubstantial.
    let me = State { offer: Some(Dir::DownLeft), ..State::AIR };
    let seen = around(&[(Dir::Up, offering(Dir::Down, DOWN))]);
    let s = next(me, State::AIR, seen, 2, 2, 1, &Physics::default());
    assert_eq!((s.material, s.accept), (Material::Sand, Some(Dir::Up)));
}

#[test]
fn resolve_an_offer_of_air_that_was_overwritten_has_no_effect() {
    // The contract's conditions absorb the change: the air I offered is gone, sand is here now.
    let kept = State { offer: Some(Dir::Down), ..State::AIR };
    let me = State { accept: Some(Dir::Up), ..sand(DOWN) };
    for accept in [Some(Dir::Up), None] {
        let seen = around(&[(Dir::Down, State { accept, ..State::AIR })]);
        let s = next(me, kept, seen, 2, 2, 0, &Physics::default());
        assert_eq!((s.material, s.velocity), (Material::Sand, DOWN));
    }
}

#[test]
fn resolve_accepted_air_spawns_a_new_air() {
    // Air offered into vacuum, accepted: it transfers, and the origin spawns itself a fresh air.
    let kept = State { offer: Some(Dir::Down), velocity: Velocity { x: 100, y: 256 }, ..State::AIR };
    let seen = around(&[(Dir::Down, State { accept: Some(Dir::Up), ..State::AIR })]);
    let s = next(State { offer: None, ..kept }, kept, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.velocity), (Material::Air, DOWN));
}

#[test]
fn evaluate_a_cell_holding_a_particle_entertains_no_offer_of_one() {
    // The capacity limit: one particle per cell. A cell holding nothing entertains any offer; one
    // holding air, which is insubstantial, an offer of a substantial particle; any other, none.
    let p = Physics::default();
    for holding in Material::ALL {
        for offered in [Material::Air, Material::Water, Material::Sand] {
            let room = holding == Material::Vacuum || (holding == Material::Air && offered != Material::Air);
            assert_eq!(p.entertains(offered, holding), room, "{offered:?} into {holding:?}");
        }
    }
}

#[test]
fn gravity_is_an_impulse_on_each_move_turning_velocity_down() {
    let p = Physics::default();
    // A particle intending to move right that moves turns toward down, at the steady scale.
    let v = Velocity { x: 256, y: 0 };
    let s = next(State::VACUUM, State::AIR, around(&[(Dir::UpLeft, offering(Dir::DownRight, v))]), 2, 2, 0, &p);
    let impulse = (p.gravity * p.mass(Material::Sand)) as i32;
    assert_eq!(s.velocity, Velocity::steady(256, impulse));
    assert!(s.velocity.y > 0 && s.velocity.x == 256, "{:?}", s.velocity);
    // Already pointing down, it stays down.
    assert_eq!(p.moved(Material::Sand, DOWN), DOWN);
    // Air is lighter: the same move turns it less.
    assert!(p.moved(Material::Air, v).y < p.moved(Material::Sand, v).y);
}

#[test]
fn resolve_accepted_into_vacuum_the_origin_is_left_vacuum() {
    let kept = offering(Dir::Down, DOWN);
    let seen = around(&[(Dir::Down, State { accept: Some(Dir::Up), ..sand(DOWN) })]);
    let s = next(sand(DOWN), kept, seen, 2, 2, 0, &Physics::default());
    assert_eq!(s.material, Material::Vacuum);
}

#[test]
fn resolve_rejected_the_origin_keeps_its_particle_and_is_deflected() {
    // The neighbor below accepted someone else; mine was rejected. My particle stays, its velocity
    // turned a little away from below, to the side it already leaned (right). I propose again.
    let p = Physics::default();
    let v = Velocity { x: 100, y: 256 };
    let kept = offering(Dir::Down, v);
    let seen = around(&[(Dir::Down, State { accept: Some(Dir::UpLeft), ..sand(DOWN) })]);
    let s = next(sand(v), kept, seen, 2, 2, 0, &p);
    assert_eq!(s.material, Material::Sand);
    assert_eq!(s.velocity, deflect(&p, Material::Sand, v, Dir::Down, 0));
    assert!(s.velocity.x > v.x && s.velocity.y == 256, "{:?}", s.velocity);
    assert!(s.offer.is_some());
}

/// A particle of `m` with velocity `v`, after the deflection effect of a rejection by the cell
/// toward `d`.
fn deflect(p: &Physics, m: Material, v: Velocity, d: Dir, bits: u32) -> Velocity {
    let mut s = State { velocity: v, ..State::holding(m) };
    Effect::Deflect { from: d, bits }.apply(&mut s, p);
    s.velocity
}

#[test]
fn contract_terms_pair_each_verdict_with_an_effect_on_each_end() {
    let offer = Offer { toward: Dir::Down };
    let origin = offering(Dir::Down, DOWN);
    assert_eq!(offer.terms(Verdict::Accepted, origin, 7), (Effect::Release, Effect::Receive { from: Dir::Up, origin }));
    // Air is insubstantial: accepted, its origin spawns a new one.
    let air = State { offer: Some(Dir::Down), ..State::AIR };
    assert_eq!(offer.terms(Verdict::Accepted, air, 7), (Effect::Spawn, Effect::Receive { from: Dir::Up, origin: air }));
    assert_eq!(offer.terms(Verdict::Rejected, origin, 7), (Effect::Deflect { from: Dir::Down, bits: 7 }, Effect::None));
    // The origin reads the verdict from the target: accepted only when the target names it.
    assert_eq!(offer.verdict(State { accept: Some(Dir::Up), ..State::AIR }), Verdict::Accepted);
    assert_eq!(offer.verdict(State { accept: Some(Dir::UpLeft), ..State::AIR }), Verdict::Rejected);
    assert_eq!(offer.verdict(State::VACUUM), Verdict::Rejected);
}

#[test]
fn deflection_turns_away_to_the_side_it_leans_or_the_hash_picks() {
    let p = Physics::default();
    // Leaning right, a rejection below turns it further right, whatever the bits.
    let v = Velocity { x: 10, y: 256 };
    for bits in [0, 1] {
        assert!(deflect(&p, Material::Water, v, Dir::Down, bits).x > 10);
    }
    // Dead on, the bits pick the side.
    let (a, b) = (deflect(&p, Material::Water, DOWN, Dir::Down, 0), deflect(&p, Material::Water, DOWN, Dir::Down, 1));
    assert!(a.x * b.x < 0, "{a:?} {b:?}");
    // Water deflects more than sand; a material with no deflection keeps its velocity.
    assert!(
        deflect(&p, Material::Water, DOWN, Dir::Down, 1).x.abs()
            > deflect(&p, Material::Sand, DOWN, Dir::Down, 1).x.abs()
    );
    let mut none = p;
    none.of_mut(Material::Sand).deflect = 0;
    assert_eq!(deflect(&none, Material::Sand, v, Dir::Down, 1), v);
    // Repeated rejections keep turning it, toward sideways.
    let mut w = DOWN;
    for _ in 0..20 {
        w = deflect(&p, Material::Water, w, Dir::Down, 1);
    }
    assert!(w.x.abs() >= w.y.abs(), "{w:?}");
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
    assert_eq!(w.material(2, 0), Material::Vacuum);
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
            let m = if hash(x, y, 0, 41) & 1 == 1 { Material::Sand } else { Material::Air };
            world.paint(x, y, m);
        }
    }
    world
}

fn materials(w: &World) -> Vec<Material> {
    (0..w.height()).flat_map(|y| (0..w.width()).map(move |x| (x, y))).map(|(x, y)| w.material(x, y)).collect()
}

#[test]
fn movement_sand_settles_through_a_packed_lattice_of_air() {
    // Air is insubstantial, so sand transfers into it: the sand sinks and the air ends up on top.
    let mut w = packed(12, 12);
    let (before, sand) = (mean_row(&w, Material::Sand), w.count(Material::Sand));
    for _ in 0..3000 {
        w.step();
    }
    assert!(mean_row(&w, Material::Sand) > before + 2.5, "{}", w.ascii());
    assert!((0..4).all(|y| (0..12).all(|x| w.material(x, y) == Material::Air)), "{}", w.ascii());
    assert_eq!(w.count(Material::Sand), sand);
}

#[test]
fn movement_air_fills_a_vacuum_cell_and_leaves_none_behind() {
    // A lattice of air with one cell of vacuum: air above offers into it, and spawns itself anew.
    let mut w = World::new(12, 12);
    w.stamp(6.0, 6.0, 20.0, Material::Air);
    w.paint(6, 6, Material::Vacuum);
    for _ in 0..20 {
        w.step();
    }
    assert_eq!(w.count(Material::Vacuum), 0, "{}", w.ascii());
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

#[test]
fn movement_a_heap_holds_its_slope_and_goes_still() {
    let mut w = World::new(40, 20);
    for p in 0..3000u32 {
        if p % 12 == 0 && p < 1800 {
            w.stamp(20.0, 1.0, 0.8, Material::Sand);
        }
        w.step();
    }
    let before = materials(&w);
    for _ in 0..50 {
        w.step();
    }
    assert_eq!(materials(&w), before, "{}", w.ascii());
    // A heap, not a layer: the middle column stands above the edges. (Sand's small deflection
    // lowered it from 6 rows of rise to 4 in v01.05.)
    let top = |x: i32| (0..20).find(|&y| w.material(x, y) == Material::Sand).unwrap_or(20);
    assert!(top(20) + 3 < top(5), "{}", w.ascii());
}

// Slate

#[test]
fn propose_nothing_offers_into_slate() {
    let slate = State::holding(Material::Slate);
    // Vacuum only straight up, where velocity never reaches.
    let seen = [State::VACUUM, slate, slate, slate, slate, slate];
    let p = Physics::default();
    for m in [Material::Sand, Material::Water] {
        let n = proposals(State::holding(m), seen, &p);
        assert_eq!(n[6], 4000, "{m:?} proposes nothing: {n:?}");
    }
}

#[test]
fn movement_slate_holds_sand_up() {
    // A slate shelf across the middle: sand poured above it stays above it.
    let mut w = World::new(20, 20);
    for x in 0..20 {
        w.paint(x, 10, Material::Slate);
    }
    w.stamp(10.0, 4.0, 2.5, Material::Sand);
    let grains = w.count(Material::Sand);
    for _ in 0..800 {
        w.step();
    }
    let below =
        (11..20).flat_map(|y| (0..20).map(move |x| (x, y))).filter(|&(x, y)| w.material(x, y) == Material::Sand);
    assert_eq!(below.count(), 0, "{}", w.ascii());
    assert_eq!((w.count(Material::Sand), w.count(Material::Slate)), (grains, 20));
}

// Water

fn water(velocity: Velocity) -> State {
    State { velocity, ..State::holding(Material::Water) }
}

#[test]
fn propose_water_still_prefers_down() {
    let n = proposals(water(DOWN), [State::VACUUM; 6], &Physics::default());
    assert!(
        n[Dir::Down as usize] > n[Dir::DownLeft as usize] && n[Dir::DownLeft as usize] > n[Dir::UpLeft as usize] * 4,
        "{n:?}"
    );
}

#[test]
fn evaluate_water_rejects_sand_nothing_is_swapped_or_displaced() {
    // Sand may offer into water (its intent), but a cell holding a particle rejects every offer of one.
    let seen = around(&[(Dir::Up, offering(Dir::Down, DOWN))]);
    let s = next(water(DOWN), State::AIR, seen, 2, 2, 0, &Physics::default());
    assert_eq!((s.material, s.accept), (Material::Water, None));
    assert!(Physics::default().offers_to(Material::Sand, DOWN, Dir::Down, Material::Water));
}

#[test]
fn movement_sand_rests_on_water_and_water_never_climbs() {
    // A pool of water under a layer of sand: the sand can't swap with or displace the water, so
    // the water stays where it is.
    let mut w = World::new(16, 16);
    for x in 0..16 {
        for y in 6..10 {
            w.paint(x, y, Material::Sand);
        }
        for y in 10..16 {
            w.paint(x, y, Material::Water);
        }
    }
    let water_top = |w: &World| (0..16).find(|&y| (0..16).any(|x| w.material(x, y) == Material::Water)).unwrap();
    for _ in 0..600 {
        w.step();
        assert_eq!(water_top(&w), 10, "{}", w.ascii());
    }
}

#[test]
fn movement_air_spreads_to_fill_vacuum() {
    // A cloud of air in empty space: each air offered into vacuum and accepted spawns a new one, so
    // the air fills the space, from the floor up.
    let mut w = World::new(16, 16);
    w.stamp(8.0, 4.0, 3.0, Material::Air);
    for _ in 0..600 {
        w.step();
    }
    assert!(w.count(Material::Vacuum) <= 8, "{}", w.ascii());
    assert!((8..16).all(|y| (0..16).all(|x| w.material(x, y) == Material::Air)), "{}", w.ascii());
}

#[test]
fn movement_water_seeks_its_level_where_sand_heaps() {
    // The same pour, in sand and in water: deflection on rejection spreads water into a layer, while
    // sand, deflecting little, stands in a heap.
    let pour = |m: Material| {
        let mut w = World::new(40, 20);
        for p in 0..5000u32 {
            if p % 12 == 0 && p < 1800 {
                w.stamp(20.0, 1.0, 0.8, m);
            }
            w.step();
        }
        w
    };
    let w = pour(Material::Water);
    let depth: Vec<usize> = (0..40).map(|x| (0..20).filter(|&y| w.material(x, y) == Material::Water).count()).collect();
    let (min, max) = (depth.iter().min().unwrap(), depth.iter().max().unwrap());
    assert!(max - min <= 5, "water depths {depth:?}\n{}", w.ascii());
    let s = pour(Material::Sand);
    let top = |x: i32| (0..20).find(|&y| s.material(x, y) == Material::Sand).unwrap_or(20);
    assert!(top(20) + 3 < top(5), "{}", s.ascii());
}
