//! A cell's update, stage by stage: read, resolve, evaluate, evolve, publish, propose.
//!
//! Each stage works on the cell's live state and reads only its neighbors' published state. The
//! contracts the stages execute are in `contract`; the selections in evaluate and propose are
//! biased by the physics distribution (`physics`).

use crate::selection::select;
use crate::{Dir, Physics, State, contract, hash};

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
    // An offer I published this phase is being evaluated now; its particle stays committed.
    let committed = contract::committed(me);

    resolve(&mut live, kept, &seen, x, y, phase, physics);
    if !committed {
        evaluate(&mut live, &seen, x, y, phase, physics);
    }
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
/// target's published state. I execute it: I apply the contract's effect on its origin.
#[allow(clippy::too_many_arguments)]
fn resolve(live: &mut State, kept: State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let Some(d) = kept.offer else { return };
    let verdict = contract::verdict(d, seen[d as usize]);
    contract::resolve(live, d, verdict, hash(x, y, phase, SALT_DEFLECT), physics);
}

/// Evaluate: the contracts that target me, selected one at a time, biased by the physics
/// distribution: offers it doesn't entertain are excluded, and the rest weighted (heavier particles
/// and better-aligned moves first). The selected offer is accepted, executing its effect on me.
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
    contract::accept(live, seen[i], Dir::ALL[i], physics);
}

/// Evolve: natural contracts, within the cell. None yet.
fn evolve(_live: &mut State) {}

/// Propose: the particle I host offers to move toward a neighbor, following its velocity. Start
/// with all six neighbors, exclude those whose published state the physics distribution marks
/// incompatible, and select among the rest, weighted. Offering commits the particle.
fn propose(live: &mut State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let mut weights = [0; 6];
    for d in Dir::ALL {
        let target = seen[d as usize].material;
        if physics.compatible(live.material, target) {
            weights[d as usize] = physics.weight(live.material, live.velocity, d, target);
        }
    }
    live.offer = select(&weights, hash(x, y, phase, SALT_PROPOSE)).map(|i| Dir::ALL[i]);
}
