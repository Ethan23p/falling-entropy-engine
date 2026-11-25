//! A cell's update, stage by stage: read, resolve, evaluate, evolve, publish, propose.
//!
//! The timing of one contract, an offer from A to its neighbor B:
//! - A proposes it in its update at phase `p - 1`; it is published in A's state at `p`, and A's
//!   particle is committed to it.
//! - B evaluates it in its update at `p`, and publishes the verdict at `p + 1`: accepted, naming A,
//!   or rejected, by not naming A. Meanwhile A, whose offer is still out, does nothing new.
//! - A resolves it in its update at `p + 1`, reading B's verdict. The offer is in A's kept-back
//!   state then (the one it published at `p`).
//!
//! The word a cell writes at the end of its update carries both its published state and the
//! contracts it just proposed, so neighbors read them together at the next phase.

use crate::physics::select;
use crate::{Dir, Material, Physics, State, Velocity, hash};

/// Salts for the selections, so they draw independent random bits.
pub const SALT_EVALUATE: u32 = 1;
pub const SALT_PROPOSE: u32 = 2;

/// One cell's update: its next published state from its own state, the state it kept back from
/// the phase before, and its neighbors' published state, all at the same phase (outside the
/// lattice reads as bedrock). The cell computes only itself.
#[allow(clippy::too_many_arguments)]
pub fn next(me: State, kept: State, seen: [State; 6], x: i32, y: i32, phase: u32, physics: &Physics) -> State {
    // Read: `seen` is the neighbors' published state; the live state starts as my own.
    let mut live = State { offer: None, accept: None, ..me };
    // An offer I published this phase is evaluated this phase; its particle stays committed.
    let committed = me.offer.is_some();

    resolve(&mut live, kept, &seen);
    if !committed {
        evaluate(&mut live, &seen, x, y, phase, physics);
    }
    evolve(&mut live);
    // Publish, with what propose authors.
    if !committed {
        propose(&mut live, &seen, x, y, phase, physics);
    }
    live
}

/// Resolve: the offer I published last phase has its verdict now, in the neighbor's acceptance.
/// Accepted, the particle has moved and I am granted an air. Rejected, I keep my particle and its
/// velocity.
fn resolve(live: &mut State, kept: State, seen: &[State; 6]) {
    let Some(d) = kept.offer else { return };
    if seen[d as usize].accept == Some(d.opposite()) {
        live.material = Material::Air;
        live.velocity = Velocity::ZERO;
    }
}

/// Evaluate: every offer that targets me, selected one at a time biased by the physics
/// distribution. The first whose conditions hold (I hold air, and its particle overwrites air) is
/// accepted: its particle moves in, with gravity's impulse. My capacity is then filled, so the
/// conditions of the rest fail and they are rejected, by not being named.
fn evaluate(live: &mut State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let mut weights = [0; 6];
    for d in Dir::ALL {
        let origin = seen[d as usize];
        if origin.offer == Some(d.opposite()) && physics.overwrites(origin.material, live.material) {
            weights[d as usize] = physics.weight(origin.velocity, d.opposite(), live.material).max(1);
        }
    }
    let Some(i) = select(&weights, hash(x, y, phase, SALT_EVALUATE)) else { return };
    let origin = seen[i];
    live.material = origin.material;
    live.velocity = physics.moved(origin.material, origin.velocity);
    live.accept = Some(Dir::ALL[i]);
}

/// Evolve: natural contracts, within the cell. v01.00 has none.
fn evolve(_live: &mut State) {}

/// Propose: the particle I host offers to move toward a neighbor. Start with all six, exclude those
/// whose published state the physics distribution marks incompatible, and select among the rest,
/// weighted. Offering commits the particle.
fn propose(live: &mut State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let mut weights = [0; 6];
    for d in Dir::ALL {
        let target = seen[d as usize].material;
        if physics.compatible(live.material, target) {
            weights[d as usize] = physics.weight(live.velocity, d, target);
        }
    }
    live.offer = select(&weights, hash(x, y, phase, SALT_PROPOSE)).map(|i| Dir::ALL[i]);
}
