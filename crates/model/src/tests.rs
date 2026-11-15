//! Invariants are hard checks: conservation, one verdict per proposal (both in `scenarios::run`, every
//! phase), order independence and the speed of information. Behavior assertions are measuring tools:
//! when a rule changes on purpose and an outcome moves, report it.

use crate::scenarios::{column, dump, frame, ring, run, still};
use crate::*;

fn check(result: Result<(), String>) {
    if let Err(e) = result {
        panic!("{e}");
    }
}

/// Runs a scenario up to `cap` phases, logging every frame; returns the phase it first went still.
fn settle(name: &str, w: &mut World, cap: u32) -> Option<u32> {
    let mut log = frame(w);
    let mut settled = None;
    check(run(w, cap, |w| {
        if settled.is_none() {
            log += &frame(w);
            if still(w) {
                settled = Some(w.now());
            }
        }
    }));
    dump(name, &format!("{name}: settled at phase {settled:?}\n\n{log}"));
    settled
}

// The scenarios

#[test]
fn column_1_wide_falls_through_air_and_rests() {
    let mut w = column(1);
    let settled = settle("column_1_wide", &mut w, 60);
    assert!(settled.is_some(), "never came to rest in 60 phases\n{}", w.ascii_intents());
    let bottom: Vec<Material> = (7..10).map(|y| w.owned(w.index(0, y)).material).collect();
    assert_eq!(bottom, [Material::Sand; 3], "the sand rests at the bottom\n{}", w.ascii());
}

#[test]
fn column_2_wide_falls_and_settles_without_duplicating_or_vanishing() {
    let mut w = column(2);
    let settled = settle("column_2_wide", &mut w, 80);
    assert!(settled.is_some(), "never came to rest in 80 phases\n{}", w.ascii_intents());
    let holes = w
        .cells()
        .filter(|&i| w.owned(i).is_air())
        .filter(|&i| {
            let (x, y) = w.xy(i);
            (0..y).any(|above| !w.owned(w.index(x, above)).is_air())
        })
        .count();
    assert_eq!(holes, 0, "no air under sand once still\n{}", w.ascii());
}

/// Runs a ring for `phases`, following every unit of water along it. Returns how far each unit
/// advanced, and the advances in each window of a ring's length.
fn circulate(name: &str, shape: Shape, bubbles: usize, phases: u32) -> (Vec<usize>, Vec<usize>) {
    let (mut w, order) = ring(shape, bubbles);
    let n = order.len();
    let mut places = scenarios::ring_places(&w, &order);
    let mut advanced = vec![0; places.len()];
    let mut windows = vec![0; (phases as usize).div_ceil(n)];
    let mut log = frame(&w);
    check(run(&mut w, phases, |w| {
        let now = scenarios::ring_places(w, &order);
        for (k, (&(id, was), &(id2, is))) in places.iter().zip(&now).enumerate() {
            assert_eq!(id, id2);
            let step = (is + n - was) % n;
            assert!(step <= 1, "unit {id} jumped {step} cells along the ring at phase {}\n{}", w.now(), w.ascii());
            advanced[k] += step;
            windows[(w.now() as usize - 1) / n] += step;
        }
        places = now;
        if w.now() as usize <= 2 * n {
            log += &frame(w);
        }
    }));
    let summary = format!(
        "{name}: {n} cells, {bubbles} of air, {phases} phases\nadvances per unit: {advanced:?}\nadvances per {n}-phase window: {windows:?}\n\n"
    );
    dump(name, &(summary + &log));
    (advanced, windows)
}

fn flows_forever(name: &str, shape: Shape, bubbles: usize) {
    let n = ring(shape, bubbles).1.len();
    let (advanced, windows) = circulate(name, shape, bubbles, (4 * n * n) as u32);
    assert!(advanced.iter().all(|&a| a >= n), "every unit goes round the whole ring at least once: {advanced:?}");
    assert!(windows.iter().all(|&m| m > 0), "the flow never stops: {windows:?}");
}

#[test]
fn ring_square_with_a_bubble_flows_forever() {
    flows_forever("ring_square_1_bubble", Shape::Square, 1);
}

#[test]
fn ring_hex_with_a_bubble_flows_forever() {
    flows_forever("ring_hex_1_bubble", Shape::Hex, 1);
}

#[test]
fn ring_hex_with_more_bubbles_flows_faster() {
    let total = |b| circulate(&format!("ring_hex_flux_{b}_bubbles"), Shape::Hex, b, 144).0.iter().sum::<usize>();
    let (one, two, four) = (total(1), total(2), total(4));
    assert!(one < two && two < four, "flux grows with the air in the ring: {one}, {two}, {four}");
}

/// A measurement, not a goal: one-way contracts can't move a full ring, since matter takes the place
/// only of air and no cell has room. Flowing it is the Swaps question.
#[test]
fn ring_full_holds_still_under_one_way_contracts() {
    for (name, shape) in [("ring_square_full", Shape::Square), ("ring_hex_full", Shape::Hex)] {
        let (advanced, _) = circulate(name, shape, 0, 50);
        assert!(advanced.iter().all(|&a| a == 0), "{name} moved: {advanced:?}");
    }
}

// Invariants beyond those `run` checks every phase

fn worlds() -> Vec<(&'static str, World)> {
    vec![("column 2 wide", column(2)), ("hex ring", ring(Shape::Hex, 2).0), ("square ring", ring(Shape::Square, 1).0)]
}

fn same(name: &str, how: &str, a: &World, b: &World) {
    assert!(
        a == b,
        "{name}: {how} differs at phase {}\nrow order:\n{}{how}:\n{}",
        a.now(),
        a.ascii_intents(),
        b.ascii_intents()
    );
}

#[test]
fn order_independence_any_pass_order_gives_the_same_world() {
    for (name, world) in worlds() {
        let (mut rows, mut reversed, mut shuffled) = (world.clone(), world.clone(), world.clone());
        for k in 0..40 {
            rows.step();
            reversed.step_in((0..reversed.len()).rev());
            let mut order: Vec<usize> = (0..shuffled.len()).collect();
            order.sort_by_key(|&i| hash(i as i32, 0, k, 7));
            shuffled.step_in(order);
            same(name, "reversed", &rows, &reversed);
            same(name, "shuffled", &rows, &shuffled);
        }
    }
}

#[test]
fn order_independence_asynchronous_cells_match_synchronous_ones() {
    const PHASES: u32 = 40;
    for (name, world) in worlds() {
        let mut sync = world.clone();
        for _ in 0..PHASES {
            sync.step();
        }
        // Cells update whenever their neighbors allow, picked at random, until all reach the phase.
        let mut free = world.clone();
        let mut k = 0;
        while free.cells().any(|i| free.phase(i) < PHASES) {
            let i = hash(k, 0, 0, 9) as usize % free.len();
            if free.phase(i) < PHASES && free.update(i) {
                let ahead =
                    (0..free.shape().slots()).filter_map(|s| free.neighbor(i, s)).filter(|&n| !free.is_bedrock(n));
                for n in ahead {
                    assert!(free.phase(i) <= free.phase(n) + 1, "{name}: a cell ran two phases ahead of a neighbor");
                }
            }
            k += 1;
            assert!(k < 1_000_000, "{name}: asynchronous updates stalled");
        }
        same(name, "asynchronous", &sync, &free);
    }
}

/// Cells reachable from `from` in each number of steps, through cells that update.
fn distances(w: &World, from: usize) -> Vec<u32> {
    let mut dist = vec![u32::MAX; w.len()];
    dist[from] = 0;
    let mut frontier = vec![from];
    while let Some(i) = frontier.pop() {
        for n in (0..w.shape().slots()).filter_map(|s| w.neighbor(i, s)).filter(|&n| !w.is_bedrock(n)) {
            if dist[n] > dist[i] + 1 {
                dist[n] = dist[i] + 1;
                frontier.push(n);
            }
        }
    }
    dist
}

#[test]
fn speed_of_information_at_most_one_cell_per_phase() {
    for (name, world) in worlds() {
        // Poke the world at its first unit of matter: take it away, and see how far the news gets.
        let poke = world.cells().find(|&i| !world.cell(i).matter.is_air()).unwrap();
        let (mut a, mut b) = (world.clone(), world.clone());
        let (x, y) = b.xy(poke);
        b.place(x, y, Material::Air, (0, 0));
        let dist = distances(&world, poke);
        for t in 1..=30 {
            a.step();
            b.step();
            for i in a.cells() {
                if a.cell(i) != b.cell(i) {
                    assert!(dist[i] <= t, "{name}: news reached {:?}, {} cells away, in {t} phases", a.xy(i), dist[i]);
                }
            }
        }
    }
}

#[test]
fn conservation_measured_on_published_state_counts_matter_in_transit_once() {
    let mut w = column(1);
    let expected = w.inventory();
    let mut seen_transit = false;
    for _ in 0..10 {
        w.step();
        assert_eq!(w.inventory(), expected, "owned matter is conserved\n{}", w.ascii_intents());
        if w.in_transit() > 0 {
            seen_transit = true;
            assert_eq!(w.shown().len(), expected.len() + w.in_transit(), "shown matter counts transit twice");
        }
    }
    assert!(seen_transit, "the column moves matter in transit");
}

// The update's stages, on single cells

fn water(velocity: Vector, id: u16) -> Matter {
    Matter { material: Material::Water, velocity, id }
}

fn update(me: State, kept: State, seen: [State; MAX_SLOTS]) -> State {
    let rules = Rules { gravity: 0, ..Rules::default() };
    next(Shape::Square, &rules, (0, 0, 0), me, kept, &seen)
}

fn offering(matter: Matter, s: Slot) -> State {
    let mut st = State::holding(matter);
    st.offers[s] = true;
    st
}

#[test]
fn propose_offers_along_velocity_to_a_neighbor_with_room() {
    let seen = [State::AIR; MAX_SLOTS];
    let out = update(State::holding(water((8, 0), 1)), State::AIR, seen);
    assert_eq!(out.offers[..4], [false, true, false, false], "right, its velocity's way");
}

#[test]
fn propose_skips_matter_that_is_not_moving_away() {
    let mut seen = [State::AIR; MAX_SLOTS];
    seen[1] = State::holding(water((0, 0), 2));
    let out = update(State::holding(water((8, 0), 1)), State::AIR, seen);
    assert!(!out.offers[1] && out.offers[0] != out.offers[2], "full ahead: up or down, at right angles, instead");
    seen[1] = offering(water((8, 0), 2), 1);
    let out = update(State::holding(water((8, 0), 1)), State::AIR, seen);
    assert!(out.offers[1], "matter offering itself away has room soon");
}

#[test]
fn propose_waits_while_its_last_offer_is_unanswered() {
    let out = update(offering(water((8, 0), 1), 1), State::AIR, [State::AIR; MAX_SLOTS]);
    assert!(!out.offering());
    assert_eq!(out.matter.id, 1, "still holds its matter");
}

#[test]
fn evaluate_air_accepts_one_proposal_by_priority_and_rejects_the_rest() {
    let mut seen = [State::AIR; MAX_SLOTS];
    seen[2] = offering(water((0, -8), 1), 0); // from below, heading straight here
    seen[1] = offering(water((-6, -6), 2), 3); // from the right, heading here at a slant
    let out = update(State::AIR, State::AIR, seen);
    assert_eq!(out.verdicts[2], Verdict::Accept);
    assert_eq!(out.verdicts[1], Verdict::Reject);
    assert_eq!(out.matter.id, 1, "acceptance is receipt");
    assert_eq!(out.matter.velocity, (0, -8), "turned along the way it came");
}

#[test]
fn evaluate_a_full_cell_rejects_every_proposal() {
    let mut seen = [State::AIR; MAX_SLOTS];
    seen[2] = offering(water((0, -8), 2), 0);
    let out = update(State::holding(water((0, 0), 1)), State::AIR, seen);
    assert_eq!(out.verdicts[2], Verdict::Reject);
    assert_eq!(out.matter.id, 1);
}

#[test]
fn resolve_an_accepted_giver_becomes_air_and_has_room_in_the_same_update() {
    let mut seen = [State::AIR; MAX_SLOTS];
    seen[1].verdicts[3] = Verdict::Accept; // the right neighbor took this cell's matter
    seen[1].matter = water((8, 0), 1);
    seen[2] = offering(water((0, -8), 2), 0); // and the one below offers its own
    let out = update(State::holding(water((8, 0), 1)), offering(water((8, 0), 1), 1), seen);
    assert_eq!(out.matter.id, 2, "gave its matter away and took the next");
    assert_eq!(out.verdicts[2], Verdict::Accept);
}

#[test]
fn resolve_a_rejected_giver_keeps_its_matter_and_offers_again() {
    let mut seen = [State::AIR; MAX_SLOTS];
    seen[1].verdicts[3] = Verdict::Reject;
    let out = update(State::holding(water((8, 0), 1)), offering(water((8, 0), 1), 1), seen);
    assert_eq!(out.matter.id, 1);
    assert!(out.offers[1], "the neighbor shows air now, so it offers there again");
}

// Contracts per cell

/// With one matter per cell and acceptance as receipt, offering to several neighbors duplicates
/// matter as soon as two of them accept. The conservation check catches it on a lone grain with
/// open air below. Where each target has a better offer to take (the two columns, whose straight
/// falls outrank the slanted ones) or a cell has one way to go (the ring), fanout runs the same as
/// one offer.
#[test]
fn contracts_fanout_duplicates_matter_where_a_cell_has_a_choice() {
    let fanout = |mut w: World, phases| {
        w.rules.contracts = Contracts::Fanout;
        run(&mut w, phases, |_| {})
    };
    let mut grain = World::new(Shape::Hex, 3, 10);
    grain.place(1, 0, Material::Sand, (0, 0));
    let err = fanout(grain, 10).expect_err("fanout duplicates a lone grain");
    assert!(err.contains("doubled"), "{err}");
    dump("contracts_fanout_lone_grain", &err);
    check(fanout(column(1), 40));
    check(fanout(column(2), 80));
    check(fanout(ring(Shape::Hex, 2).0, 200));
}
