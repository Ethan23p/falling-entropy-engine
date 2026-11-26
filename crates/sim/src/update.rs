//! A cell's update, stage by stage: read, resolve, evaluate, evolve, publish, propose.
//!
//! Each stage works on the cell's live state and reads only its neighbors' published state. The
//! contracts the stages execute are in `contract`; the selections in evaluate and propose are
//! biased by the physics distribution (`physics`).

use crate::contract::{Offer, Verdict};
use crate::selection::select;
use crate::{Dir, Material, Physics, State, hash};

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
    let mut live = State { offer: None, accept: None, reject: 0, ..me };
    // An offer of mine being evaluated now, or one with no verdict yet (it stands), ties up my
    // particle, and with it my capacity: I propose nothing new, and evaluate rejects every offer.
    let standing = resolve(&mut live, kept, &seen, x, y, phase, physics);
    let committed = Offer::of(me).is_some() || standing;

    evaluate(&mut live, committed, &seen, x, y, phase, physics);
    evolve(&mut live);
    // Publish, then propose. They share one write: the word a cell writes at the end of its update
    // holds both its published state and the contracts it just proposed, and neighbors read them
    // together at the next phase, as if published first.
    // On initialization, a cell that waits (every other row, `World::new`) lets its first phase pass.
    live.waits = false;
    if !committed && !me.waits {
        propose(&mut live, &seen, x, y, phase, physics);
    }
    live
}

/// Resolve: the offer I published last phase (now in my kept-back state) has its verdict, if any,
/// in the target's published state. With one, I execute it: I apply its terms' effect on me, the
/// origin. With none yet, the offer stands: I publish it again. Returns whether it stands.
#[allow(clippy::too_many_arguments)]
fn resolve(live: &mut State, kept: State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) -> bool {
    let Some(offer) = Offer::of(kept) else { return false };
    let Some(verdict) = offer.verdict(seen[offer.toward as usize]) else {
        live.offer = kept.offer;
        return true;
    };
    let (mine, _) = offer.terms(verdict, kept, hash(x, y, phase, SALT_DEFLECT));
    mine.apply(live, physics);
    false
}

/// Evaluate: the contracts that target me, selected one at a time, biased by the physics
/// distribution: offers it doesn't entertain are excluded, and the rest weighted (heavier particles
/// and better-aligned moves first). The selected offer is accepted: I apply its terms' effect on me,
/// the target. While contracts remain I select again, but a cell holds one particle at most: once
/// one is accepted, the rest are excluded. Every offer I evaluate gets a verdict: the
/// accepted one is named in `accept`, every other in `reject`. A rejected offer's terms have no
/// effect on me.
#[allow(clippy::too_many_arguments)]
fn evaluate(live: &mut State, committed: bool, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let mut weights = [0; 6];
    let mut offered = 0u8;
    for d in Dir::ALL {
        let origin = seen[d as usize];
        if origin.offer == Some(d.opposite()) {
            offered |= 1 << d as u8;
            if physics.entertains(origin.material, live.material, committed) {
                weights[d as usize] =
                    physics.weight(origin.material, origin.velocity, d.opposite(), live.material).max(1);
            }
        }
    }
    let accepted = select(&weights, hash(x, y, phase, SALT_EVALUATE));
    live.reject = offered & !accepted.map_or(0, |i| 1 << i);
    let Some(i) = accepted else { return };
    let offer = Offer { toward: Dir::ALL[i].opposite() };
    let (_, mine) = offer.terms(Verdict::Accepted, seen[i], 0);
    mine.apply(live, physics);
}

/// Evolve: natural contracts, within the cell. None yet.
fn evolve(_live: &mut State) {}

/// Propose: the particle I host offers to move toward a neighbor, following its velocity. Start
/// with all six neighbors, exclude the incompatible (`Physics::offers_to`), and select among the
/// rest, weighted. Air with vacuum in its neighborhood offers into the vacuum
/// (`Physics::air_meets_vacuum`). Offering commits the particle.
fn propose(live: &mut State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let mut weights = [0; 6];
    let only_vacuum = physics.air_meets_vacuum(live.material, seen);
    for d in Dir::ALL {
        let target = seen[d as usize].material;
        if only_vacuum && target != Material::Vacuum {
            continue;
        }
        if physics.offers_to(live.material, target) {
            weights[d as usize] = physics.weight(live.material, live.velocity, d, target);
        }
    }
    live.offer = select(&weights, hash(x, y, phase, SALT_PROPOSE)).map(|i| Dir::ALL[i]);
}
