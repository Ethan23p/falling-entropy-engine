//! Contracts: negotiations between two neighboring cells, directed from an origin (where the
//! particle comes from) to a target (where it goes). A contract is authored by one end and
//! evaluated by the other, during its evaluate stage; the author resolves it once it has a
//! verdict. It deals in resources and capacities, never creates or loses any, and each end
//! applies its effects to itself only.
//!
//! A contract has **conditions**, which decide which of its **effects** each end applies:
//! - **Accepted:** the particle moves. The target takes it, with gravity's impulse (it made a
//!   physical move), and the origin is left holding nothing.
//! - **Rejected:** the particle stays with the origin. Provisionally, its velocity is deflected.
//!
//! Which offers a target entertains at all is the physics distribution's to say
//! (`Physics::entertains`), not the contract's.
//!
//! Only **offers** are built so far: contracts authored by the origin, which pre-commit the particle
//! they propose to transfer. The doc's **requests** (authored by the target, reserving its capacity)
//! and **matched pairs** (one contract proposed by both ends in the same phase, accepted
//! automatically and resolved on both ends) aren't built yet.
//!
//! The timing of an offer from A to its neighbor B, in published state:
//! - A proposes it in its update at phase `p - 1`: A publishes `offer` (toward B) at `p`, and its
//!   particle is committed.
//! - B evaluates it in its update at `p`, and publishes its verdict at `p + 1`: `accept` naming A,
//!   or a rejection, by not naming A. Meanwhile A, committed, neither evaluates nor proposes.
//! - A resolves it in its update at `p + 1`, reading B's verdict. The offer is in A's kept-back
//!   state then (the one it published at `p`).
//!
//! While the particle is in transport, both cells register as holding it; a census counts it once,
//! at its origin (`State::counts_as`).

use crate::{Dir, Material, Physics, State, Velocity};

/// The verdict on a contract, as its author reads it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Accepted,
    Rejected,
}

/// Whether `me` has an offer out: its particle is committed to it, so the cell neither evaluates
/// nor proposes until the offer resolves.
pub fn committed(me: State) -> bool {
    me.offer.is_some()
}

/// The verdict on an offer toward `d`, read from the target's published state: accepted if it names
/// this cell, rejected otherwise.
pub fn verdict(d: Dir, target: State) -> Verdict {
    if target.accept == Some(d.opposite()) { Verdict::Accepted } else { Verdict::Rejected }
}

/// The offer's effect on its target, on acceptance: the particle from the origin toward `from`
/// arrives with gravity's impulse, and the target's verdict names that origin.
pub fn accept(target: &mut State, origin: State, from: Dir, physics: &Physics) {
    target.material = origin.material;
    target.velocity = physics.moved(origin.material, origin.velocity);
    target.accept = Some(from);
}

/// The offer's effect on its origin, at resolve. Accepted: the particle has left, and the origin
/// holds nothing. Rejected: the origin keeps its particle, deflected (provisional).
pub fn resolve(origin: &mut State, d: Dir, verdict: Verdict, bits: u32, physics: &Physics) {
    match verdict {
        Verdict::Accepted => *origin = State::VACUUM,
        Verdict::Rejected => origin.velocity = deflected(physics, origin.material, origin.velocity, d, bits),
    }
}

const DEFLECT_CAP: u32 = 1 << 10;

/// Provisional: the rejection effect. A stand-in within the model that lets a rejected particle
/// change its intent: its velocity turns away from the target `d`, perpendicular to it, by its
/// material's deflection, to the side it already leans (or, pointing dead on, to the side `bits`
/// picks), then back to the steady scale. Every rejection gets this same effect, whatever the
/// reason for it.
pub fn deflected(physics: &Physics, m: Material, v: Velocity, d: Dir, bits: u32) -> Velocity {
    let k = match m {
        Material::Air => physics.deflect_air,
        Material::Water => physics.deflect_water,
        Material::Sand => physics.deflect_sand,
        _ => 0,
    }
    .min(DEFLECT_CAP) as i32;
    if k == 0 {
        return v;
    }
    let (sx, sy) = d.step();
    // The perpendicular (-sy, sx), and how far the velocity already leans along it.
    let lean = -(v.x as i32) * sy + v.y as i32 * sx;
    let side = if lean > 0 || (lean == 0 && bits & 1 == 1) { 1 } else { -1 };
    Velocity::steady(v.x as i32 - side * k * sy / 2, v.y as i32 + side * k * sx / 2)
}
