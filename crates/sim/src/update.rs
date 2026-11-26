//! A cell's update, stage by stage: read, resolve, evaluate, evolve, publish, propose.
//!
//! Each stage works on the cell's live state and reads only its neighbors' published state. The
//! contracts the stages execute are in `contract`; the selections in evaluate and propose are
//! biased by the physics distribution (`physics`).

use crate::contract::{Offer, Verdict};
use crate::selection::select;
use crate::{Dir, Physics, State, hash};

/// Salts for the selections, so they draw independent random bits.
pub const SALT_EVALUATE: u32 = 1;
pub const SALT_PROPOSE: u32 = 2;
pub const SALT_DEFLECT: u32 = 3;

/// One cell's update: its next published state from its own state, the state it kept back from
/// the phase before, and its neighbors' published state, all at the same phase (outside the
/// lattice reads as bedrock). The cell computes only itself.
#[allow(clippy::too_many_arguments)]
pub fn next(me: State, kept: State, seen: [State; 6], x: i32, y: i32, phase: u32, physics: &Physics) -> State {
    // Read: `seen` is the neighbors' published state; the live state starts as my own, its
    // contracts cleared until the stages below write them again.
    let mut live = State { offer: None, accept: None, ..me };
    // An offer I published this phase is being evaluated now; its particle stays committed, so I
    // propose nothing new. I still evaluate: what I can take is the capacity limit's to say.
    let committed = Offer::of(me).is_some();

    resolve(&mut live, kept, &seen, x, y, phase, physics);
    evaluate(&mut live, &seen, x, y, phase, physics);
    evolve(&mut live);
    // Publish, then propose. They share one write: the word a cell writes at the end of its update
    // holds both its published state and the contracts it just proposed, and neighbors read them
    // together at the next phase, as if published first.
    if !committed {
        propose(&mut live, &seen, x, y, phase, physics);
    }
    live
}

/// Resolve: the offer I published last phase (now in my kept-back state) has its verdict in the
/// target's published state. I execute it: I apply its terms' effect on me, the origin.
#[allow(clippy::too_many_arguments)]
fn resolve(live: &mut State, kept: State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let Some(offer) = Offer::of(kept) else { return };
    let verdict = offer.verdict(seen[offer.toward as usize]);
    // The terms absorb a change in context: if the particle I offered is no longer here (air,
    // overwritten while its offer was out), the offer has no effect on me.
    if live.material != kept.material {
        return;
    }
    let (mine, _) = offer.terms(verdict, kept, hash(x, y, phase, SALT_DEFLECT));
    mine.apply(live, physics);
}

/// Evaluate: the contracts that target me, selected one at a time, biased by the physics
/// distribution: offers it doesn't entertain are excluded, and the rest weighted (heavier particles
/// and better-aligned moves first). The selected offer is accepted: I apply its terms' effect on me,
/// the target. A rejected offer's terms have no effect on me.
/// While contracts remain I would select again, but a cell holds one particle at most: once one is
/// accepted, the capacity limit excludes the rest, and they are rejected by not being named.
fn evaluate(live: &mut State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let mut weights = [0; 6];
    for d in Dir::ALL {
        let origin = seen[d as usize];
        if origin.offer == Some(d.opposite()) && physics.entertains(origin.material, live.material) {
            weights[d as usize] = physics.weight(origin.material, origin.velocity, d.opposite(), live.material).max(1);
        }
    }
    let Some(i) = select(&weights, hash(x, y, phase, SALT_EVALUATE)) else { return };
    let offer = Offer { toward: Dir::ALL[i].opposite() };
    let (_, mine) = offer.terms(Verdict::Accepted, seen[i], 0);
    mine.apply(live, physics);
}

/// Evolve: natural contracts, within the cell. None yet.
fn evolve(_live: &mut State) {}

/// Propose: the particle I host offers to move toward a neighbor, following its velocity. Start
/// with all six neighbors, exclude those it wouldn't offer to (`Physics::offers_to`), and select
/// among the rest, weighted. Offering commits the particle.
fn propose(live: &mut State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let mut weights = [0; 6];
    for d in Dir::ALL {
        let target = seen[d as usize].material;
        if physics.offers_to(live.material, target) {
            weights[d as usize] = physics.weight(live.material, live.velocity, d, target);
        }
    }
    live.offer = select(&weights, hash(x, y, phase, SALT_PROPOSE)).map(|i| Dir::ALL[i]);
}
