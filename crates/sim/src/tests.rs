//! Rule tests, named after the primitives and policies they pin.

use crate::*;

/// The rules before velocity: matter free to fall offers at once, and every fall earns speed. The
/// contract tests run on these; the motion tests below run on the shipped rules.
const INSTANT: Rules = Rules { start: [1 << 16; 4], gravity: 1 << 16, splash: TOP_SPEED, friction: 1 << 16 };

/// An empty world on the instant rules.
fn instant(w: i32, h: i32) -> World {
    let mut world = World::new(w, h);
    world.rules = INSTANT;
    world
}

/// A world with sand at each `(x, y)`.
fn world(w: i32, h: i32, sand: &[(i32, i32)]) -> World {
    let mut world = instant(w, h);
    for &(x, y) in sand {
        world.paint(x, y, Material::Sand);
    }
    world
}

fn sand_at(world: &World) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for y in 0..world.height() {
        for x in 0..world.width() {
            if world.looks(x, y) == Material::Sand {
                out.push((x, y));
            }
        }
    }
    out
}

/// A scattering of sand over the top half: a deterministic pseudo-random fill.
fn scattered(w: i32, h: i32) -> World {
    let mut world = instant(w, h);
    for y in 0..h / 2 {
        for x in 0..w {
            if hash(x, y, 0, 99).is_multiple_of(3) {
                world.paint(x, y, Material::Sand);
            }
        }
    }
    world
}

/// One update of a cell with nothing kept back: no offer waiting on its answer.
fn fresh(me: State, seen: [State; 6], x: i32, y: i32, phase: u32, rules: &Rules) -> State {
    next(me, State::AIR, seen, x, y, phase, rules)
}

/// The rejection bits for the given directions.
fn rejecting(dirs: &[Dir]) -> u8 {
    dirs.iter().fold(0, |r, &d| r | 1 << d as u8)
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

// Contracts: offer, answer (acceptance is receipt), clear

/// What a cell sees: air all around, except the given neighbors.
fn seen(with: &[(Dir, State)]) -> [State; 6] {
    let mut out = [State::AIR; 6];
    for &(d, s) in with {
        out[d as usize] = s;
    }
    out
}

/// Sand offering toward `d`, on terms that nothing comes back.
fn offering(d: Dir) -> State {
    State { material: Material::Sand, offer: Some(d), ..State::AIR }
}

/// Sand offering toward `d` on terms that the water there comes back: a trade.
fn trading(d: Dir) -> State {
    State { terms: Material::Water, ..offering(d) }
}

/// Matter just received from `from`.
fn received(m: Material, from: Dir) -> State {
    State { material: m, accept: Some(from), ..State::AIR }
}

#[test]
fn offer_sand_offers_straight_down_into_air() {
    let mut w = world(5, 5, &[(2, 1)]);
    w.step();
    assert_eq!((w.cell(2, 1).offer, w.cell(2, 1).terms), (Some(Dir::Down), Material::Air));
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
fn offer_promised_matter_is_neither_offered_again_nor_takes_anything() {
    // I offered down this phase; the answer comes next phase. Meanwhile my grain is promised: I
    // make no new offer, and an offer into me is rejected.
    let me = offering(Dir::Down);
    let s = fresh(me, seen(&[(Dir::Up, offering(Dir::Down))]), 2, 2, 0, &INSTANT);
    assert_eq!((s.material, s.offer, s.accept), (Material::Sand, None, None));
    assert_eq!(s.rejects, rejecting(&[Dir::Up]));
    // Promised water takes no trade either.
    let me = State { offer: Some(Dir::UpRight), ..water(Side::Right) };
    let s = fresh(me, blocked_below(&[(Dir::Up, trading(Dir::Down))]), 2, 2, 0, &INSTANT);
    assert_eq!((s.material, s.accept, s.rejects), (Material::Water, None, rejecting(&[Dir::Up])));
}

#[test]
fn offer_a_cell_offers_every_other_phase_at_most() {
    // Offer, wait for the answer, offer again: a cell never has two offers out.
    let mut w = world(3, 30, &[(1, 0)]);
    for _ in 0..30 {
        w.step();
        for y in 0..30 {
            assert!(!(w.kept(1, y).offer.is_some() && w.cell(1, y).offer.is_some()), "{}", w.ascii_intents());
        }
    }
}

#[test]
fn contract_acceptance_is_receipt_naming_the_giver() {
    let mut w = world(5, 5, &[(2, 1)]);
    w.step(); // offer
    w.step(); // accept
    let r = w.cell(2, 2);
    assert_eq!((r.material, r.accept), (Material::Sand, Some(Dir::Up)), "{}", w.ascii_intents());
    // The giver still shows the grain until it reads the acceptance, but the grain is the receiver's.
    assert_eq!(w.material(2, 1), Material::Sand);
    assert_eq!(w.taker(2, 1), Some(Dir::Down));
    assert_eq!((w.looks(2, 1), w.count(Material::Sand)), (Material::Air, 1));
}

#[test]
fn contract_the_giver_clears_on_its_next_update() {
    let mut w = world(5, 5, &[(2, 1)]);
    for _ in 0..3 {
        w.step();
    }
    assert_eq!(w.cell(2, 1), State::AIR, "{}", w.ascii_intents());
}

#[test]
fn contract_a_receiver_offers_onward_in_the_same_update() {
    let mut w = world(5, 5, &[(2, 1)]);
    w.step();
    w.step();
    assert_eq!(w.cell(2, 2).offer, Some(Dir::Down), "{}", w.ascii_intents());
}

#[test]
fn contract_every_offer_gets_an_explicit_answer() {
    // Three grains offer into one cell of air: it accepts one and rejects the other two, by name.
    let around = seen(&[
        (Dir::Up, offering(Dir::Down)),
        (Dir::UpLeft, offering(Dir::DownRight)),
        (Dir::UpRight, offering(Dir::DownLeft)),
    ]);
    for phase in 0..16 {
        let s = fresh(State::AIR, around, 2, 2, phase, &INSTANT);
        assert_eq!((s.accept, s.rejects), (Some(Dir::Up), rejecting(&[Dir::UpLeft, Dir::UpRight])));
    }
}

#[test]
fn contract_an_offer_that_cant_be_taken_is_rejected() {
    // Matter offered into a cell holding matter, or on terms it can't meet, is turned down.
    let into = |me: State, with: State| fresh(me, seen(&[(Dir::Up, with)]), 2, 2, 0, &INSTANT);
    for me in [State::holding(Material::Sand), State::SLATE, State::BEDROCK] {
        let s = into(me, offering(Dir::Down));
        assert_eq!((s.accept, s.rejects), (None, rejecting(&[Dir::Up])), "{me:?}");
    }
    // Air can't send water back, and water taking matter must send its water back.
    let s = into(State::AIR, trading(Dir::Down));
    assert_eq!((s.material, s.rejects), (Material::Air, rejecting(&[Dir::Up])));
    let s = into(water(Side::Left), offering(Dir::Down));
    assert_eq!((s.material, s.rejects), (Material::Water, rejecting(&[Dir::Up])));
}

#[test]
fn contract_an_accepted_offer_clears_to_its_terms() {
    // I offered down last phase (kept back), and the cell below names me: I clear to air.
    let kept = offering(Dir::Down);
    let around = seen(&[(Dir::Down, received(Material::Sand, Dir::Up))]);
    assert_eq!(next(State::holding(Material::Sand), kept, around, 2, 2, 0, &INSTANT), State::AIR);
    // On a trade's terms, to water, still.
    let s = next(State::holding(Material::Sand), trading(Dir::Down), around, 2, 2, 0, &INSTANT);
    assert_eq!((s.material, s.speed, s.accept), (Material::Water, 0, None));
}

#[test]
fn contract_a_rejected_offer_leaves_the_matter_free() {
    // My offer down-right was turned down: my grain is mine again, free to offer elsewhere now.
    let kept = offering(Dir::DownRight);
    let target = State { rejects: rejecting(&[Dir::UpLeft]), ..received(Material::Sand, Dir::Up) };
    let around = seen(&[(Dir::DownRight, target), (Dir::Down, SLATE)]);
    let s = next(State::holding(Material::Sand), kept, around, 2, 2, 0, &INSTANT);
    assert_eq!((s.material, s.offer), (Material::Sand, Some(Dir::DownLeft)));
}

#[test]
fn accept_straight_above_beats_a_diagonal() {
    let around = seen(&[(Dir::Up, offering(Dir::Down)), (Dir::UpLeft, offering(Dir::DownRight))]);
    for phase in 0..32 {
        for x in [2, 3] {
            let s = fresh(State::AIR, around, x, 2, phase, &INSTANT);
            assert_eq!((s.accept, s.rejects), (Some(Dir::Up), rejecting(&[Dir::UpLeft])));
        }
    }
}

#[test]
fn accept_diagonal_ties_are_hashed_and_pick_exactly_one() {
    let around = seen(&[(Dir::UpLeft, offering(Dir::DownRight)), (Dir::UpRight, offering(Dir::DownLeft))]);
    let picks: Vec<_> = (0..64).map(|phase| fresh(State::AIR, around, 2, 2, phase, &INSTANT)).collect();
    assert!(picks.iter().all(|s| s.rejects.count_ones() == 1));
    let picks: Vec<_> = picks.iter().map(|s| s.accept).collect();
    assert!(picks.iter().all(|p| matches!(p, Some(Dir::UpLeft | Dir::UpRight))));
    assert!(picks.contains(&Some(Dir::UpLeft)) && picks.contains(&Some(Dir::UpRight)));
}

#[test]
fn accept_ignores_offers_aimed_elsewhere() {
    let around = seen(&[(Dir::UpLeft, offering(Dir::Down))]);
    let s = fresh(State::AIR, around, 2, 2, 0, &INSTANT);
    assert_eq!((s.accept, s.rejects), (None, 0));
}

#[test]
fn contract_a_fall_takes_a_cell_every_phase_at_the_speed_of_light() {
    let mut w = world(3, 12, &[(1, 0)]);
    w.step(); // the first offer
    for y in 1..12 {
        w.step();
        assert_eq!(sand_at(&w), vec![(1, y)], "{}", w.ascii_intents());
    }
}

#[test]
fn contract_a_falling_column_spaces_out_to_every_fourth_cell() {
    // A grain's cell is free only once it has read its offer accepted and cleared, two phases after
    // offering, and the grain above sees that the phase after. So a falling column spaces out to a
    // grain every fourth cell, and no wider; then it piles back up on the floor.
    let mut w = World::new(1, 60);
    w.rules = INSTANT;
    for y in 0..6 {
        w.paint(0, y, Material::Sand);
    }
    let mut widest = 0;
    for phase in 0..400 {
        w.step();
        let ys: Vec<i32> = sand_at(&w).iter().map(|&(_, y)| y).collect();
        let gap = ys.windows(2).map(|p| p[1] - p[0]).max().unwrap();
        assert!(gap <= 4, "phase {phase}: {ys:?}\n{}", w.ascii_intents());
        widest = widest.max(gap);
    }
    assert_eq!(widest, 4);
    assert_eq!(sand_at(&w), (54..60).map(|y| (0, y)).collect::<Vec<_>>());
}

#[test]
fn contract_grain_comes_to_rest_on_the_floor() {
    let mut w = world(3, 6, &[(1, 0)]);
    for _ in 0..40 {
        w.step();
    }
    assert_eq!(sand_at(&w), vec![(1, 5)]);
    assert_eq!((w.cell(1, 5).offer, w.cell(1, 5).accept), (None, None));
}

#[test]
fn contract_audit_catches_an_acceptance_of_nothing_offered() {
    let mut w = instant(5, 5);
    w.put(2, 2, received(Material::Sand, Dir::Up));
    let err = w.audit().unwrap_err();
    assert!(err.contains("never made"), "{err}");
}

#[test]
fn contract_audit_catches_two_owners() {
    // (2, 1)'s grain, offered down, accepted both below it and down-right of it.
    let mut w = instant(5, 5);
    w.put_kept(2, 1, offering(Dir::Down));
    w.put(2, 1, State::holding(Material::Sand));
    w.put(2, 2, received(Material::Sand, Dir::Up));
    w.put(3, 1, received(Material::Sand, Dir::UpLeft));
    let err = w.audit().unwrap_err();
    assert!(err.contains("never made"), "{err}");
}

#[test]
fn contract_audit_catches_an_unanswered_offer() {
    let mut w = instant(5, 5);
    w.put_kept(2, 1, offering(Dir::Down));
    w.put(2, 1, State::holding(Material::Sand));
    let err = w.audit().unwrap_err();
    assert!(err.contains("unanswered"), "{err}");
    // Or answered twice.
    w.put(2, 2, State { rejects: rejecting(&[Dir::Up]), ..received(Material::Sand, Dir::Up) });
    let err = w.audit().unwrap_err();
    assert!(err.contains("both"), "{err}");
}

#[test]
fn contract_audit_catches_promised_matter_offered_again() {
    let mut w = instant(5, 5);
    w.put_kept(2, 1, offering(Dir::Down));
    w.put(2, 1, offering(Dir::DownLeft));
    w.put(2, 2, State { rejects: rejecting(&[Dir::Up]), ..State::AIR });
    let err = w.audit().unwrap_err();
    assert!(err.contains("already promised"), "{err}");
}

#[test]
fn contract_audit_catches_an_offer_without_matter() {
    let mut w = instant(5, 5);
    w.put(2, 1, State { offer: Some(Dir::Down), ..State::AIR });
    let err = w.audit().unwrap_err();
    assert!(err.contains("doesn't hold"), "{err}");
}

#[test]
fn contract_audit_passes_a_busy_world() {
    // `step` audits after every step in debug builds; this one does it by hand, so a release
    // build checks it too.
    let mut w = scattered(24, 24);
    for _ in 0..200 {
        w.step();
        w.audit().unwrap();
    }
}

#[test]
fn contract_random_worlds_keep_the_rules() {
    // Worlds of every material, on the instant and the shipped rules: after every step, every unit
    // of matter has one owner, no offer exists without its matter, and every offer has one answer.
    for seed in 0..120u32 {
        let (wd, ht) = (4 + (seed % 9) as i32, 4 + (seed / 9 % 9) as i32);
        let mut w = World::new(wd, ht);
        if seed % 2 == 0 {
            w.rules = INSTANT;
        }
        for y in 0..ht {
            for x in 0..wd {
                let m = [Material::Air, Material::Sand, Material::Water, Material::Slate]
                    [(hash(x, y, seed, 31) % 4) as usize];
                w.paint(x, y, m);
            }
        }
        let counts = [Material::Sand, Material::Water].map(|m| w.count(m));
        for _ in 0..120 {
            w.step();
            if let Err(e) = w.audit() {
                panic!("seed {seed}: {e}");
            }
        }
        assert_eq!([Material::Sand, Material::Water].map(|m| w.count(m)), counts, "seed {seed}");
    }
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
    let mut w = instant(16, 16);
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
                assert_eq!(w.looks(nx, ny), Material::Sand, "air below ({x},{y}) toward {d:?}");
            }
        }
    }
}

#[test]
fn piling_a_column_spreads_into_a_pile() {
    let mut w = instant(31, 20);
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
    let mut a = pool(16, 16, 6);
    for y in 0..8 {
        for x in 0..16 {
            if hash(x, y, 0, 21).is_multiple_of(3) {
                a.paint(x, y, Material::Sand);
            }
        }
    }
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
    assert_eq!(a, b);
}

// Brush

#[test]
fn brush_paints_and_erases() {
    let mut w = instant(4, 4);
    w.paint(1, 1, Material::Sand);
    assert_eq!(w.count(Material::Sand), 1);
    w.paint(1, 1, Material::Air);
    assert_eq!(w.count(Material::Sand), 0);
}

#[test]
fn brush_erasing_a_falling_grain_takes_it_whole() {
    // At top speed a grain is often mid-contract: received in one cell, its giver above not yet
    // cleared. Erasing the receiver leaves air still naming the giver, so the giver clears rather
    // than take the grain back.
    let mut w = world(5, 8, &[(2, 1)]);
    w.step(); // offer
    w.step(); // accept
    w.paint(2, 2, Material::Air);
    assert_eq!(w.cell(2, 2), State { accept: Some(Dir::Up), ..State::AIR });
    assert_eq!((w.looks(2, 1), w.looks(2, 2), w.count(Material::Sand)), (Material::Air, Material::Air, 0));
    w.paint(2, 2, Material::Sand);
    assert_eq!(w.material(2, 2), Material::Air, "not painted until the giver has cleared");
    for _ in 0..10 {
        w.step();
        assert_eq!(w.count(Material::Sand), 0, "{}", w.ascii_intents());
    }
}

#[test]
fn brush_leaves_an_accepted_giver_alone() {
    // A giver whose offer was accepted reads as air but still shows the grain until it clears;
    // painting or erasing it would undo or duplicate a finished send.
    for m in [Material::Air, Material::Sand, Material::Water, Material::Slate] {
        let mut w = world(5, 8, &[(2, 1)]);
        w.step();
        w.step();
        let giver = w.cell(2, 1);
        w.paint(2, 1, m);
        assert_eq!(w.cell(2, 1), giver, "{m:?}");
        w.step();
        assert_eq!(w.count(Material::Sand), 1);
    }
}

#[test]
fn brush_keeps_a_cells_answers() {
    // Three grains offer into one cell; painting it before the answers are read keeps them, so
    // every offer still has its answer.
    let mut w = world(5, 6, &[(1, 0), (2, 0), (3, 0)]);
    for x in [1, 3] {
        w.put(x, 1, State::SLATE);
    }
    w.put(2, 2, State::SLATE);
    w.step(); // offers
    w.step(); // answers, at (2, 1)
    let answered = w.cell(2, 1);
    assert!(answered.rejects != 0 || answered.accept.is_some(), "{}", w.ascii_intents());
    w.paint(2, 1, Material::Air);
    let erased = w.cell(2, 1);
    assert_eq!((erased.accept, erased.rejects), (answered.accept, answered.rejects));
    w.step();
}

// Encoding

#[test]
fn encoding_state_round_trips() {
    let dirs = [None, Some(Dir::Up), Some(Dir::DownRight), Some(Dir::UpLeft)];
    let mut states = vec![State::AIR, State::SLATE, State::BEDROCK];
    for material in [Material::Air, Material::Sand, Material::Water, Material::Slate] {
        for offer in dirs {
            for terms in [Material::Air, Material::Water] {
                for accept in dirs {
                    for rejects in [0, 1, 0b10_1010, 63] {
                        for speed in 0..=TOP_SPEED {
                            for flow in [Side::Left, Side::Right] {
                                states.push(State { material, offer, terms, accept, rejects, speed, flow });
                            }
                        }
                    }
                }
            }
        }
    }
    for s in states {
        assert_eq!(State::decode(s.encode()), s);
        assert!(s.encode() < 1 << 19, "19 bits");
    }
}

#[test]
fn encoding_a_cell_keeps_a_word_per_phase_parity() {
    // The state of phase p goes in word p % 2, tagged with p, so the word a neighbor at p reads
    // stays put until p + 2.
    let mut w = world(3, 3, &[(1, 0)]);
    const { assert!(STATE_MASK == (1 << 19) - 1 && PHASE_MASK == (1 << 13) - 1) };
    w.step();
    let words = w.words();
    let (even, odd) = (words[2], words[3]);
    assert_eq!((even >> 19, odd >> 19), (0, 1));
    assert_eq!(State::decode(even & STATE_MASK), State::holding(Material::Sand));
    assert_eq!(State::decode(odd & STATE_MASK).offer, Some(Dir::Down));
}

#[test]
fn encoding_the_older_word_holds_the_offer_waited_on() {
    let mut w = world(3, 4, &[(1, 0)]);
    w.step(); // offer
    w.step(); // answered: the offer is now in the kept-back word
    assert_eq!((w.kept(1, 0).offer, w.cell(1, 0).offer), (Some(Dir::Down), None));
    assert_eq!(w.cell(1, 1).accept, Some(Dir::Up));
}

// Slate

const SLATE: State = State::SLATE;

fn slate_world(w: i32, h: i32, slate: &[(i32, i32)], sand: &[(i32, i32)]) -> World {
    let mut world = instant(w, h);
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
fn slate_never_offers_or_accepts_and_rejects_what_is_offered() {
    let around = seen(&[(Dir::Up, offering(Dir::Down))]);
    for phase in 0..8 {
        assert_eq!(fresh(SLATE, around, 2, 2, phase, &INSTANT), State { rejects: rejecting(&[Dir::Up]), ..SLATE });
        assert_eq!(fresh(SLATE, seen(&[]), 2, 2, phase, &INSTANT), SLATE);
    }
}

#[test]
fn offer_sand_never_offers_to_slate() {
    let walled = seen(&[(Dir::Down, SLATE), (Dir::DownLeft, SLATE), (Dir::DownRight, SLATE)]);
    assert_eq!(fresh(State::holding(Material::Sand), walled, 2, 2, 0, &INSTANT).offer, None);
    let gap = seen(&[(Dir::Down, SLATE), (Dir::DownRight, SLATE)]);
    assert_eq!(fresh(State::holding(Material::Sand), gap, 2, 2, 0, &INSTANT).offer, Some(Dir::DownLeft));
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
    let mut w = instant(4, 4);
    w.paint(1, 1, Material::Sand);
    w.paint(1, 1, Material::Slate);
    assert_eq!(w.material(1, 1), Material::Sand, "slate does not replace sand");
    w.paint(2, 2, Material::Slate);
    w.paint(2, 2, Material::Sand);
    assert_eq!(w.material(2, 2), Material::Slate, "sand does not replace slate");
}

#[test]
fn brush_stamp_covers_a_disc_of_centres() {
    let mut w = instant(9, 9);
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
    let mut open = instant(w, h);
    for y in 0..h {
        for x in 0..w {
            if hash(x, y, 0, 13).is_multiple_of(2) {
                open.paint(x, y, Material::Sand);
            }
        }
    }
    let mut framed = instant(w + 1, h + 1);
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
    let bedrock = State::BEDROCK;
    let around = seen(&[(Dir::Up, offering(Dir::Down))]);
    assert_eq!(fresh(bedrock, around, 2, 2, 0, &INSTANT), State { rejects: rejecting(&[Dir::Up]), ..bedrock });
    let below = seen(&[(Dir::Down, bedrock), (Dir::DownLeft, bedrock), (Dir::DownRight, bedrock)]);
    assert_eq!(fresh(State::holding(Material::Sand), below, 2, 2, 0, &INSTANT).offer, None);
}

#[test]
fn brush_cannot_erase_bedrock() {
    let mut w = instant(4, 4);
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
            if world.looks(x, y) == Material::Water {
                out.push((x, y));
            }
        }
    }
    out
}

/// Water with speed to spend, so its side steps are paid for.
fn water(flow: Side) -> State {
    State { material: Material::Water, flow, speed: TOP_SPEED, ..State::AIR }
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
    let mut w = instant(5, 5);
    w.paint(2, 1, Material::Water);
    w.step(); // offer
    w.step(); // accept
    assert_eq!(water_at(&w), vec![(2, 2)]);
    w.step();
    assert_eq!(water_at(&w), vec![(2, 3)]);
}

#[test]
fn water_falls_to_its_flow_side_when_both_diagonals_are_open() {
    for flow in [Side::Left, Side::Right] {
        let around = seen(&[(Dir::Down, SLATE)]);
        let want = if flow == Side::Left { Dir::DownLeft } else { Dir::DownRight };
        for phase in 0..16 {
            assert_eq!(fresh(water(flow), around, 2, 2, phase, &INSTANT).offer, Some(want));
        }
    }
}

#[test]
fn step_water_that_cannot_fall_offers_to_its_flow_side() {
    let s = fresh(water(Side::Right), blocked_below(&[]), 2, 2, 0, &INSTANT);
    assert_eq!((s.offer, s.terms, s.flow), (Some(Dir::UpRight), Material::Air, Side::Right));
    assert_eq!(s.speed, TOP_SPEED - 1, "an unpushed step is paid for");
    let s = fresh(water(Side::Left), blocked_below(&[]), 2, 2, 0, &INSTANT);
    assert_eq!((s.offer, s.flow), (Some(Dir::UpLeft), Side::Left));
}

#[test]
fn step_takes_the_other_side_when_its_flow_side_is_shut() {
    let s = fresh(water(Side::Right), blocked_below(&[(Dir::UpRight, SLATE)]), 2, 2, 0, &INSTANT);
    assert_eq!((s.offer, s.flow), (Some(Dir::UpLeft), Side::Left));
}

#[test]
fn step_sand_never_moves_sideways() {
    let s = fresh(State::holding(Material::Sand), blocked_below(&[]), 2, 2, 0, &INSTANT);
    assert_eq!(s.offer, None);
}

#[test]
fn step_air_accepts_water_only_where_it_can_fall_on() {
    // The giver is down-left, offering up-right; the far lower diagonal is down-right.
    let giver = (Dir::DownLeft, water_offering(Dir::UpRight, Side::Right));
    for x in [2, 3] {
        let shut = seen(&[giver, (Dir::Down, SLATE), (Dir::DownRight, SLATE)]);
        let s = fresh(State::AIR, shut, x, 2, 0, &INSTANT);
        assert_eq!((s.accept, s.rejects), (None, rejecting(&[Dir::DownLeft])), "level: rejected");
        let far_open = seen(&[giver, (Dir::Down, SLATE)]);
        assert_eq!(fresh(State::AIR, far_open, x, 2, 0, &INSTANT).accept, Some(Dir::DownLeft));
        let down_open = seen(&[giver, (Dir::DownRight, SLATE)]);
        assert_eq!(fresh(State::AIR, down_open, x, 2, 0, &INSTANT).accept, Some(Dir::DownLeft));
    }
}

#[test]
fn step_side_offers_lose_to_falls() {
    let around = seen(&[
        (Dir::DownLeft, water_offering(Dir::UpRight, Side::Right)),
        (Dir::UpRight, water_offering(Dir::DownLeft, Side::Left)),
    ]);
    for phase in 0..32 {
        let s = fresh(State::AIR, around, 2, 2, phase, &INSTANT);
        assert_eq!((s.accept, s.rejects), (Some(Dir::UpRight), rejecting(&[Dir::DownLeft])));
    }
}

#[test]
fn step_rejected_flips_the_flow() {
    // My step up-right was turned down: I flip, and offer up-left in the same update.
    let kept = water_offering(Dir::UpRight, Side::Right);
    let refusal = State { rejects: rejecting(&[Dir::DownLeft]), ..State::AIR };
    let around = blocked_below(&[(Dir::UpRight, refusal)]);
    let s = next(water(Side::Right), kept, around, 2, 2, 1, &INSTANT);
    assert_eq!((s.offer, s.flow), (Some(Dir::UpLeft), Side::Left));
}

#[test]
fn contract_receipt_carries_water_and_its_flow() {
    // Received from straight above: the flow carries over.
    let s = fresh(State::AIR, seen(&[(Dir::Up, water_offering(Dir::Down, Side::Left))]), 2, 2, 0, &INSTANT);
    assert_eq!((s.material, s.flow, s.accept), (Material::Water, Side::Left, Some(Dir::Up)));
    // Received from up-left, moving right: the flow becomes right.
    let around = seen(&[(Dir::UpLeft, water_offering(Dir::DownRight, Side::Left))]);
    let s = fresh(State::AIR, around, 2, 2, 0, &INSTANT);
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
    let mut w = instant(16, 12);
    ramp(&mut w);
    w.put(0, 11, water(Side::Right));
    for _ in 0..300 {
        w.step();
        assert_eq!(water_at(&w), vec![(0, 11)]);
    }
}

#[test]
fn water_runs_along_a_floor_in_a_line_until_its_speed_is_spent() {
    // A moving drop on the floor keeps its flow: it covers ground in a line, not as a random walk,
    // and each level step spends speed, so it comes to rest.
    let mut w = instant(48, 4);
    w.put(1, 3, water(Side::Right));
    for _ in 0..120 {
        w.step();
    }
    let [(x, _)] = water_at(&w)[..] else { panic!("one drop") };
    assert!(x >= 3, "only reached column {x}");
    for _ in 0..200 {
        w.step();
        assert_eq!(water_at(&w), vec![(x, 3)], "at rest");
    }
}

#[test]
fn water_spreads_and_comes_to_rest() {
    // Pools of a few widths, each let go from one side, reach every column and come to rest:
    // nothing the observer draws moves. How level the rest is, is open (water leveling waits on
    // pressure and momentum passed by contract), so this pins only that it gets there.
    for wd in 20..28 {
        let ht = 10;
        let mut w = World::new(wd, ht);
        for y in 1..ht {
            for x in 0..7 {
                w.paint(x, y, Material::Water);
            }
        }
        for _ in 0..3000 {
            w.step();
        }
        let rested = water_at(&w);
        for _ in 0..300 {
            w.step();
            assert_eq!(water_at(&w), rested, "width {wd}: still moving");
        }
        assert_eq!(rested.len(), 63, "width {wd}: water not conserved");
        for x in 0..wd {
            assert!((0..ht).any(|y| w.looks(x, y) == Material::Water), "width {wd}: column {x} dry\n{}", w.ascii());
        }
    }
}

#[test]
fn water_level_surface_is_still() {
    // A pool filled exactly level stays put: no surface churn.
    let mut w = instant(12, 8);
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

// Trade (displacement)

/// A pool: water filling the bottom `depth` rows.
fn pool(w: i32, h: i32, depth: i32) -> World {
    let mut world = instant(w, h);
    for y in h - depth..h {
        for x in 0..w {
            world.paint(x, y, Material::Water);
        }
    }
    world
}

#[test]
fn trade_sand_offers_into_still_water_on_terms_that_the_water_comes_back() {
    let s = fresh(State::holding(Material::Sand), seen(&[(Dir::Down, water(Side::Left))]), 2, 2, 0, &INSTANT);
    assert_eq!((s.offer, s.terms), (Some(Dir::Down), Material::Water));
}

#[test]
fn trade_sand_never_offers_to_moving_water() {
    let around = seen(&[
        (Dir::Down, water_offering(Dir::DownLeft, Side::Left)),
        (Dir::DownLeft, SLATE),
        (Dir::DownRight, SLATE),
    ]);
    assert_eq!(fresh(State::holding(Material::Sand), around, 2, 2, 0, &INSTANT).offer, None);
}

#[test]
fn trade_still_water_accepts_sand_and_holds_it() {
    let s = fresh(water(Side::Right), blocked_below(&[(Dir::Up, trading(Dir::Down))]), 2, 2, 0, &INSTANT);
    assert_eq!((s.material, s.accept, s.rejects, s.speed), (Material::Sand, Some(Dir::Up), 0, 0));
}

#[test]
fn trade_water_under_sand_waits_for_it_rather_than_step_aside() {
    // Pushed by the grain, the water could step aside; it holds still so the grain can sink into it.
    for above in [Dir::Up, Dir::UpLeft, Dir::UpRight] {
        let around = blocked_below(&[(above, State::holding(Material::Sand))]);
        assert_eq!(fresh(water(Side::Right), around, 2, 2, 0, &INSTANT).offer, None, "{above:?}");
    }
}

#[test]
fn trade_a_grain_sinks_into_a_level_surface() {
    let mut w = pool(9, 6, 2);
    w.paint(4, 3, Material::Sand);
    for _ in 0..40 {
        w.step();
    }
    let [(x, 5)] = sand_at(&w)[..] else { panic!("the grain didn't sink\n{}", w.ascii_intents()) };
    assert!((3..=5).contains(&x));
}

#[test]
fn trade_water_never_accepts_water() {
    let into_water = State { terms: Material::Water, ..water_offering(Dir::Down, Side::Left) };
    for offer in [water_offering(Dir::Down, Side::Left), into_water] {
        let s = fresh(water(Side::Left), blocked_below(&[(Dir::Up, offer)]), 2, 2, 0, &INSTANT);
        assert_eq!((s.material, s.accept, s.rejects), (Material::Water, None, rejecting(&[Dir::Up])));
    }
}

/// Steps until the water at (2, 4) takes the grain above it. Pushed by the grain at rest, it may
/// first be stepping aside itself, and takes nothing while that matter is promised.
fn take(w: &mut World) {
    for _ in 0..8 {
        w.step();
        if w.cell(2, 4).accept == Some(Dir::Up) {
            return;
        }
    }
    panic!("the water never took the grain\n{}", w.ascii_intents());
}

#[test]
fn trade_sand_and_water_swap_with_no_air_between() {
    // A grain on a level pool one row deep: it offers, the water takes it, and the grain's old cell
    // clears to water by the offer's terms.
    let mut w = pool(5, 5, 1);
    w.paint(2, 3, Material::Sand);
    take(&mut w);
    let below = w.cell(2, 4);
    assert_eq!((below.material, below.accept), (Material::Sand, Some(Dir::Up)));
    assert_eq!(w.kept(2, 3).terms, Material::Water);
    let looks = (w.looks(2, 3), w.looks(2, 4));
    assert_eq!(looks, (Material::Water, Material::Sand), "the giver reads as the water coming back");
    assert_eq!((w.count(Material::Sand), w.count(Material::Water)), (1, 5));
    w.step(); // the giver clears to water
    let gap = w.cell(2, 3);
    assert_eq!((gap.material, gap.accept), (Material::Water, None), "{}", w.ascii_intents());
    assert_eq!((w.count(Material::Sand), w.count(Material::Water)), (1, 5));
}

#[test]
fn trade_never_opens_air_in_a_pool() {
    let mut w = pool(9, 14, 10);
    for x in 2..7 {
        w.paint(x, 1, Material::Sand);
    }
    for _ in 0..200 {
        w.step();
        for y in 4..14 {
            for x in 0..9 {
                assert_ne!(w.looks(x, y), Material::Air, "({x},{y})\n{}", w.ascii_intents());
            }
        }
    }
}

#[test]
fn trade_sand_sinks_to_the_floor_of_a_pool() {
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
fn trade_sand_piles_under_water() {
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
    // Settled: no grain has air or water downhill.
    for (x, y) in sand_at(&w) {
        for d in [Dir::Down, Dir::DownLeft, Dir::DownRight] {
            let (nx, ny) = neighbor(x, y, d);
            if w.in_bounds(nx, ny) {
                assert!(
                    !matches!(w.looks(nx, ny), Material::Air | Material::Water),
                    "grain at ({x},{y}) could still sink {d:?}\n{}",
                    w.ascii()
                );
            }
        }
    }
}

#[test]
fn trade_water_never_sinks_into_sand() {
    let mut w = instant(10, 10);
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
fn trade_brush_leaves_the_giver_alone() {
    // A grain sinking through deep water: its giver, accepted, reads as the water coming back
    // until it clears. If the brush could fill or clear it, that water would be lost or doubled.
    for m in [Material::Water, Material::Sand, Material::Slate, Material::Air] {
        let mut w = instant(5, 6);
        for y in 1..6 {
            for x in 0..5 {
                w.paint(x, y, Material::Water);
            }
        }
        w.paint(2, 0, Material::Sand);
        let mut tried = 0;
        for _ in 0..100 {
            w.step();
            for y in 0..6 {
                if w.taker(2, y).is_some() && w.looks(2, y) == Material::Water {
                    let giver = w.cell(2, y);
                    w.paint(2, y, m);
                    assert_eq!(w.cell(2, y), giver, "{m:?}: the brush leaves the giver alone");
                    tried += 1;
                }
            }
            assert_eq!(w.count(Material::Water), 25);
        }
        assert!(tried > 3);
        let [(_, y)] = sand_at(&w)[..] else { panic!("one grain") };
        assert_eq!(y, 5, "{m:?}: the grain reaches the floor\n{}", w.ascii_intents());
    }
}

#[test]
fn trade_brush_erasing_the_receiver_takes_only_the_grain() {
    // Mid-trade the water is already owed back to the giver by the terms, so erasing the cell that
    // took the grain removes the grain and nothing else.
    let mut w = pool(5, 5, 1);
    w.paint(2, 3, Material::Sand);
    take(&mut w);
    w.paint(2, 4, Material::Air);
    assert_eq!((w.looks(2, 3), w.looks(2, 4)), (Material::Water, Material::Air));
    for _ in 0..20 {
        w.step();
        assert_eq!((w.count(Material::Sand), w.count(Material::Water)), (0, 5), "{}", w.ascii_intents());
    }
}

#[test]
fn trade_settles_under_a_busy_brush() {
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
        // Settled: no grain has air or water straight below.
        for (x, y) in sand_at(&w) {
            if y + 1 < w.height() {
                let below = w.looks(x, y + 1);
                assert!(
                    !matches!(below, Material::Air | Material::Water),
                    "{m:?}: ({x},{y}) still sinking\n{}",
                    w.ascii()
                );
            }
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
    let mut w = instant(32, 32);
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
    let mut a = instant(20, 20);
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
    let mut w = instant(4, 4);
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
    let mut w = instant(16, 4);
    for x in 0..16 {
        w.paint(x, 1, Material::Water);
    }
    let flows: Vec<_> = (0..16).map(|x| w.cell(x, 1).flow).collect();
    assert!(flows.contains(&Side::Left) && flows.contains(&Side::Right));
}

// Speed

/// Rules with the given chances (out of 65,536) and splash.
fn rules(start: u32, gravity: u32, splash: u8, friction: u32) -> Rules {
    Rules { start: [start; 4], gravity, splash, friction }
}

const ALWAYS: u32 = 1 << 16;

fn moving(m: Material, d: Dir, speed: u8) -> State {
    State { material: m, offer: Some(d), speed, ..State::AIR }
}

/// The phases a lone grain takes to cross each cell of its fall.
fn fall_times(world: &mut World) -> Vec<u32> {
    let (mut times, mut last_y, mut last_phase) = (Vec::new(), 0, 0);
    for phase in 1..400 {
        world.step();
        let [(_, y)] = sand_at(world)[..] else { panic!("one grain") };
        if y != last_y {
            times.push(phase - last_phase);
            (last_y, last_phase) = (y, phase);
        }
    }
    times
}

#[test]
fn speed_starts_slow_and_builds_to_a_cell_every_phase() {
    let mut w = World::new(3, 40);
    w.paint(1, 0, Material::Sand);
    let times = fall_times(&mut w);
    assert_eq!(times.len(), 39, "reaches the floor: {times:?}");
    let first: u32 = times[..4].iter().sum();
    assert!(first > 4 + 4, "starts slow: {times:?}");
    assert!(times[20..].iter().all(|&t| t == 1), "top speed is the speed of light: {times:?}");
    assert_eq!(w.cell(1, 39).speed, 0, "landed");
}

#[test]
fn speed_a_cell_at_top_speed_offers_at_once() {
    let fast = State { speed: TOP_SPEED, ..State::holding(Material::Sand) };
    let still = State::holding(Material::Sand);
    let never = rules(0, 0, 0, 0);
    let mut table = never;
    table.start[TOP_SPEED as usize] = ALWAYS;
    for phase in 0..32 {
        assert_eq!(fresh(fast, seen(&[]), 2, 2, phase, &table).offer, Some(Dir::Down));
        assert_eq!(fresh(still, seen(&[]), 2, 2, phase, &table).offer, None, "lingers");
    }
}

#[test]
fn speed_the_start_chance_gates_only_free_falls_into_air() {
    // Never starting on its own, a grain still slides off a diagonal and trades into water.
    let never = rules(0, 0, 0, 0);
    let sand = State::holding(Material::Sand);
    let slide = seen(&[(Dir::Down, SLATE), (Dir::DownLeft, SLATE)]);
    assert_eq!(fresh(sand, slide, 2, 2, 0, &never).offer, Some(Dir::DownRight));
    let sink = seen(&[(Dir::Down, State::holding(Material::Water))]);
    assert_eq!(fresh(sand, sink, 2, 2, 0, &never).offer, Some(Dir::Down));
}

#[test]
fn contract_receipt_carries_speed_and_a_fall_earns() {
    let me = State::AIR;
    let above = seen(&[(Dir::Up, moving(Material::Sand, Dir::Down, 1))]);
    assert_eq!(fresh(me, above, 2, 2, 0, &rules(ALWAYS, ALWAYS, 0, 0)).speed, 2, "earned");
    assert_eq!(fresh(me, above, 2, 2, 0, &rules(ALWAYS, 0, 0, 0)).speed, 1, "carried");
    let top = seen(&[(Dir::Up, moving(Material::Sand, Dir::Down, TOP_SPEED))]);
    assert_eq!(fresh(me, top, 2, 2, 0, &rules(ALWAYS, ALWAYS, 0, 0)).speed, TOP_SPEED, "bounded");
    // A diagonal fall earns on half the chance.
    let diagonal = seen(&[(Dir::UpLeft, moving(Material::Sand, Dir::DownRight, 1))]);
    let earned = (0..256).filter(|&p| fresh(me, diagonal, 2, 2, p, &rules(ALWAYS, ALWAYS, 0, 0)).speed == 2).count();
    assert!((96..160).contains(&earned), "{earned} of 256");
}

#[test]
fn speed_sand_stops_dead_when_it_lands() {
    let me = State { speed: TOP_SPEED, ..State::holding(Material::Sand) };
    assert_eq!(fresh(me, blocked_below(&[]), 2, 2, 0, &Rules::default()).speed, 0);
}

#[test]
fn speed_water_keeps_at_most_the_splash_when_it_lands() {
    // Walled in on both sides too, so it can't step aside and spend any.
    let walled = blocked_below(&[(Dir::UpLeft, SLATE), (Dir::UpRight, SLATE)]);
    for splash in 0..=TOP_SPEED {
        let s = fresh(water(Side::Left), walled, 2, 2, 0, &rules(ALWAYS, 0, splash, 0));
        assert_eq!(s.speed, splash);
    }
}

#[test]
fn speed_an_unpushed_side_step_needs_speed_and_spends_it() {
    let costly = rules(ALWAYS, 0, TOP_SPEED, ALWAYS);
    let resting = State { speed: 0, ..water(Side::Right) };
    assert_eq!(fresh(resting, blocked_below(&[]), 2, 2, 0, &costly).offer, None, "rests");
    let s = fresh(State { speed: 2, ..resting }, blocked_below(&[]), 2, 2, 0, &costly);
    assert_eq!((s.offer, s.speed), (Some(Dir::UpRight), 1));
    // The cost falls on the friction chance.
    let slick = rules(ALWAYS, 0, TOP_SPEED, 0);
    assert_eq!(fresh(State { speed: 2, ..resting }, blocked_below(&[]), 2, 2, 0, &slick).speed, 2);
}

#[test]
fn speed_a_pushed_side_step_is_free() {
    // Water above, or above on the side it leaves, pushes it on even at rest.
    let costly = rules(ALWAYS, 0, TOP_SPEED, ALWAYS);
    let resting = State { speed: 0, ..water(Side::Right) };
    for pusher in [Dir::Up, Dir::UpLeft] {
        let around = blocked_below(&[(pusher, State::holding(Material::Water))]);
        let s = fresh(resting, around, 2, 2, 0, &costly);
        assert_eq!((s.offer, s.speed), (Some(Dir::UpRight), 0), "pushed from {pusher:?}");
    }
    // Matter moving into it doesn't push: matter offered to it, or the giver it just took from,
    // which still shows the water that is now its own.
    let leaving = blocked_below(&[(Dir::UpLeft, water_offering(Dir::DownRight, Side::Right))]);
    assert_eq!(fresh(resting, leaving, 2, 2, 0, &costly).offer, None);
    let took = State { accept: Some(Dir::UpLeft), ..resting };
    let giver = blocked_below(&[(Dir::UpLeft, State::holding(Material::Water))]);
    assert_eq!(fresh(took, giver, 2, 2, 0, &costly).offer, None);
}

#[test]
fn speed_a_trade_drags_it_to_zero() {
    let fast = State { terms: Material::Water, ..moving(Material::Sand, Dir::Down, TOP_SPEED) };
    let s = fresh(water(Side::Left), blocked_below(&[(Dir::Up, fast)]), 2, 2, 0, &Rules::default());
    assert_eq!((s.material, s.speed), (Material::Sand, 0));
}

#[test]
fn speed_restless_drops_come_to_rest_on_a_level_pool() {
    // Drops rained onto a level pool land with speed, skate a little, and stop.
    let mut w = pool(40, 20, 6);
    for x in (3..37).step_by(3) {
        w.paint(x, 2, Material::Water);
    }
    let water = w.count(Material::Water);
    let (mut still, mut before) = (0, water_at(&w));
    for _ in 0..3000 {
        w.step();
        let now = water_at(&w);
        still = if now == before { still + 1 } else { 0 };
        before = now;
    }
    assert!(still >= 600, "still moving after 3000 phases\n{}", w.ascii());
    assert_eq!(w.count(Material::Water), water);
}

#[test]
fn synchrony_velocity_is_independent_of_evaluation_order() {
    let mut a = pool(24, 24, 8);
    for y in 0..14 {
        for x in 0..24 {
            match hash(x, y, 0, 11) % 3 {
                0 => a.paint(x, y, Material::Sand),
                1 => a.paint(x, y, Material::Water),
                _ => {}
            }
        }
    }
    let mut b = a.clone();
    let cells: Vec<_> = (0..24).flat_map(|y| (0..24).map(move |x| (x, y))).collect();
    for phase in 0..300 {
        a.step();
        let mut order = cells.clone();
        order.sort_by_key(|&(x, y)| hash(x, y, phase, 12));
        for (x, y) in order {
            assert!(b.update(x, y));
        }
        assert_eq!(a, b, "phase {phase}");
    }
}

#[test]
fn conservation_holds_with_velocity() {
    let mut w = pool(32, 32, 10);
    for y in 0..18 {
        for x in 0..32 {
            match hash(x, y, 0, 13) % 3 {
                0 => w.paint(x, y, Material::Sand),
                1 => w.paint(x, y, Material::Water),
                _ => {}
            }
        }
    }
    let (sand, water) = (w.count(Material::Sand), w.count(Material::Water));
    for _ in 0..1500 {
        w.step();
        assert_eq!((w.count(Material::Sand), w.count(Material::Water)), (sand, water));
    }
}
