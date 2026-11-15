//! Invariants are hard checks: conservation, one verdict per proposal (both in `scenarios::run`, every
//! phase), order independence and the speed of information. Behavior assertions are measuring tools:
//! when a rule changes on purpose and an outcome moves, report it.

use crate::scenarios::{Start, column, dump, frame, moved, ring, run};
use crate::*;

fn check(result: Result<(), String>) {
    if let Err(e) = result {
        panic!("{e}");
    }
}

/// Runs a scenario `phases` phases with every invariant checked, logging the first `logged` frames.
/// Returns the last phase in which any matter moved.
fn observe(name: &str, w: &mut World, phases: u32, logged: u32) -> u32 {
    let mut log = frame(w);
    let mut last_move = 0;
    check(run(w, phases, |w| {
        if moved(w) {
            last_move = w.now();
        }
        if w.now() <= logged {
            log += &frame(w);
        }
    }));
    let end = format!(
        "after {phases} phases:
{}",
        frame(w)
    );
    dump(
        name,
        &format!(
            "{name}: last move at phase {last_move}

{end}
{log}"
        ),
    );
    last_move
}

// The scenarios: run on the CPU reference with every invariant checked; what they do is in the dumps.

/// Whether the column's matter lies with no air under it in its column.
fn holes(w: &World) -> usize {
    w.cells()
        .filter(|&i| w.owned(i).is_air())
        .filter(|&i| {
            let (x, y) = w.xy(i);
            (0..y).any(|above| !w.owned(w.index(x, above)).is_air())
        })
        .count()
}

fn column_runs(name: &str, width: i32, material: Material) {
    let mut w = column(width, material);
    let last = observe(name, &mut w, 200, 40);
    let holes = holes(&w);
    eprintln!("{name}: last move at phase {last}, {holes} cells of air under matter");
}

#[test]
fn column_1_wide_sand() {
    column_runs("column_1_wide_sand", 1, Material::Sand);
}

#[test]
fn column_1_wide_water() {
    column_runs("column_1_wide_water", 1, Material::Water);
}

#[test]
fn column_2_wide_sand() {
    column_runs("column_2_wide_sand", 2, Material::Sand);
}

#[test]
fn column_2_wide_water() {
    column_runs("column_2_wide_water", 2, Material::Water);
}

/// Runs a ring for `phases`, following every unit of water along it. Returns each unit's moves
/// clockwise and counterclockwise.
fn circulate(name: &str, shape: Shape, start: Start, phases: u32) -> Vec<(usize, usize)> {
    let (mut w, order) = ring(shape, start);
    let n = order.len();
    let mut places = scenarios::ring_places(&w, &order);
    let mut advanced = vec![(0, 0); places.len()];
    let mut log = frame(&w);
    check(run(&mut w, phases, |w| {
        let now = scenarios::ring_places(w, &order);
        for (k, (&(id, was), &(id2, is))) in places.iter().zip(&now).enumerate() {
            assert_eq!(id, id2);
            let step = (is + n - was) % n;
            assert!(
                step <= 1 || step == n - 1,
                "unit {id} jumped {step} cells along the ring at phase {}
{}",
                w.now(),
                w.ascii()
            );
            match step {
                1 => advanced[k].0 += 1,
                0 => {}
                _ => advanced[k].1 += 1,
            }
        }
        places = now;
        if w.now() as usize <= 2 * n {
            log += &frame(w);
        }
    }));
    let summary = format!(
        "{name}: {n} cells, {start:?}, {phases} phases
moves per unit (clockwise, counterclockwise): {advanced:?}

"
    );
    dump(name, &(summary + &log));
    advanced
}

#[test]
fn ring_square_full() {
    circulate("ring_square_full", Shape::Square, Start::Full, 256);
}

#[test]
fn ring_hex_full() {
    circulate("ring_hex_full", Shape::Hex, Start::Full, 144);
}

#[test]
fn ring_square_flowing() {
    circulate("ring_square_flowing", Shape::Square, Start::Flowing, 256);
}

#[test]
fn ring_hex_flowing() {
    circulate("ring_hex_flowing", Shape::Hex, Start::Flowing, 144);
}

// Invariants beyond those `run` checks every phase

fn worlds() -> Vec<(&'static str, World)> {
    vec![
        ("column 2 wide", column(2, Material::Sand)),
        ("water column", column(2, Material::Water)),
        ("hex ring", ring(Shape::Hex, Start::Flowing).0),
        ("square ring", ring(Shape::Square, Start::Flowing).0),
    ]
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
    let mut w = column(1, Material::Sand);
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
    next(Shape::Square, &Rules::default(), (0, 0, 0), me, kept, &seen)
}

fn offering(matter: Matter, s: Slot) -> State {
    let mut st = State::holding(matter);
    st.offers[s] = true;
    st
}

#[test]
fn propose_moving_matter_keeps_its_direction_and_takes_its_size_from_density() {
    let out = update(State::holding(water((40, 0), 1)), State::AIR, [State::AIR; MAX_SLOTS]);
    assert_eq!(out.matter.velocity, (8, 0), "water's density times gravity, the way it was going");
    assert_eq!(out.offers[..4], [false, true, false, false], "right, its velocity's way");
}

#[test]
fn propose_at_rest_offers_to_any_neighbor_at_random() {
    let mut seen = [State::holding(water((0, 0), 2)); MAX_SLOTS];
    seen[0] = State::BEDROCK;
    let out = update(State::holding(water((0, 0), 1)), State::AIR, seen);
    assert_eq!(out.matter.velocity, (0, 0), "nothing lighter below or heavier above: no velocity");
    assert!(out.offering() && !out.offers[0], "every neighbor ties, bedrock aside: one at random");
}

#[test]
fn propose_dense_over_lighter_is_nudged_perpendicular() {
    let mut seen = [State::holding(water((0, 0), 2)); MAX_SLOTS];
    seen[2] = State::AIR;
    let out = update(State::holding(water((0, 0), 1)), State::AIR, seen);
    assert_eq!(out.matter.velocity.1, 0, "perpendicular to the density below: sideways");
    assert_eq!(out.matter.velocity.0.abs(), 8, "size from density against gravity");
    assert!(out.offers[1] || out.offers[3]);
}

#[test]
fn propose_light_under_heavier_is_nudged_strongly_perpendicular() {
    let mut seen = [State::holding(water((0, 0), 2)); MAX_SLOTS];
    seen[0] = State::holding(Matter { material: Material::Sand, velocity: (0, 0), id: 3 });
    let out = update(State::holding(water((0, 8), 1)), State::AIR, seen);
    let v = out.matter.velocity;
    assert!(v.0.abs() > v.1, "the strong nudge outweighs the carried velocity: {v:?}");
}

#[test]
fn propose_offers_into_matter_too() {
    let seen = [State::holding(water((0, 0), 2)); MAX_SLOTS];
    let out = update(State::holding(water((0, 8), 1)), State::AIR, seen);
    assert!(out.offers[2], "water offers to water: the target decides");
}

#[test]
fn propose_never_to_bedrock_which_exerts_nothing() {
    let mut seen = [State::BEDROCK; MAX_SLOTS];
    seen[1] = State::AIR;
    let out = update(State::holding(water((0, 8), 1)), State::AIR, seen);
    assert_eq!(out.matter.velocity, (0, 8), "bedrock below exerts nothing");
    assert!(!out.offers[2], "not into the bedrock its velocity points at");
    assert!(out.offers[1], "the smallest turn that is not bedrock");
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
    assert!(out.offering(), "it offers again");
}

#[test]
#[ignore]
fn trace_ring_velocities() {
    let (mut w, order) = ring(Shape::Square, Start::Flowing);
    for _ in 0..256 {
        let row: Vec<String> = order
            .iter()
            .map(|&i| {
                let c = w.cell(i);
                let m = w.owned(i);
                if m.is_air() {
                    format!("{:?}:.", w.xy(i))
                } else {
                    format!("{:?}:{:?}{}", w.xy(i), c.matter.velocity, if c.offering() { "*" } else { "" })
                }
            })
            .collect();
        eprintln!("p{} {}", w.now(), row.join(" "));
        w.step();
    }
}
