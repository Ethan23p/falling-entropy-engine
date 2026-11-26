//! Contracts: negotiations between two neighboring cells, directed from an origin (where the
//! particle comes from) to a target (where it goes). A contract is authored by one end and
//! evaluated by the other, during its evaluate stage; the author resolves it once it has a
//! verdict. It deals in resources and capacities, never creates or loses any, and each end
//! applies its effects to itself only.
//!
//! A contract's **terms** pair each of its **conditions** with the **effects** each end applies.
//! For an offer of a particle, the condition is the verdict (`Offer::terms`):
//!
//! | Condition | Effect on the origin | Effect on the target |
//! |---|---|---|
//! | Accepted | `Release`: holds nothing; or for air, `Spawn`: holds a new air | `Receive`: holds the particle, with gravity's impulse |
//! | Rejected | `Deflect`: keeps the particle; its intent turns away from the target | none |
//!
//! Air and vacuum interact specially: a cell holding air offers it into vacuum (the only cell that
//! takes air), and on acceptance transfers it and spawns itself a new air. Air is insubstantial,
//! so this is no breach of conservation; the census counts only substantial particles.
//!
//! Which offers a target entertains at all is the physics distribution's to say
//! (`Physics::entertains`), not the contract's. How much an effect does is the materials' data
//! (`physics::Properties`): a rejected sand particle deflects a little (forced on down a hill),
//! water a lot (it spreads on collision). Deflection is a first cut, to iterate on.
//!
//! A **request** is the mirror: authored by the target, asking the origin to transfer its particle
//! in. It can't pre-commit that particle; what it ties up is the requester's capacity
//! (`Request::terms`):
//!
//! | Condition | Effect on the origin (the holder) | Effect on the target (the requester) |
//! |---|---|---|
//! | Accepted | `Give`: its particle is committed to the transfer; then, when it resolves, `Release` (or `Spawn` for air) | `Take`: holds the particle, with gravity's impulse |
//! | Rejected | none | none: its capacity is free again |
//!
//! A **matched pair** is one transfer proposed by both ends in the same phase: an offer from A to B
//! while B requests from A. It is accepted automatically: B accepts the offer, and A gives the
//! request no verdict of its own.
//!
//! The timing of an offer from A to its neighbor B, in published state:
//! - A proposes it in its update at phase `p - 1`: A publishes `offer` (toward B) at `p`, and its
//!   particle is committed.
//! - B evaluates it in its update at `p`, and publishes its verdict at `p + 1`: `accept` naming A,
//!   or `reject` naming A. Every offer B evaluates gets one or the other. Meanwhile A, its particle
//!   tied up in the offer and its capacity with it, proposes nothing new, and rejects every offer
//!   into it.
//! - A resolves it in its update at `p + 1`, reading B's verdict. The offer is in A's kept-back
//!   state then (the one it published at `p`).
//!
//! While the particle is in transport, both cells register as holding it; a census counts it once,
//! at its origin (`State::counts_as`).
//!
//! The timing of a request from B to its neighbor A, in published state:
//! - B proposes it in its update at phase `p - 1`: B publishes `request` (toward A) at `p`, and its
//!   capacity is tied up.
//! - A evaluates it in its update at `p`, and publishes its verdict at `p + 1`: `accept` naming B
//!   with `gives` (still holding the particle, committed to B), or `reject` naming B. Meanwhile B
//!   proposes nothing new, and rejects every offer into it but a matching one.
//! - Both resolve in their updates at `p + 1`: B reads A's verdict from A's published state, and on
//!   acceptance takes the particle A still publishes; A, reading its own `gives`, lets it go.

use crate::{Dir, Physics, State, Velocity};

/// An offer: a contract its origin authors, to transfer its particle to the neighbor `toward`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Offer {
    pub toward: Dir,
}

/// A request: a contract its target authors, asking the neighbor `from` to transfer its particle in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Request {
    pub from: Dir,
}

/// A contract's condition: the verdict on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Accepted,
    Rejected,
}

/// What one end of a contract applies to itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Nothing changes.
    None,
    /// The particle leaves: the cell holds nothing.
    Release,
    /// The particle leaves, and the cell spawns itself a new particle of the same material: a fresh
    /// one, intending down. Only for an insubstantial particle (air).
    Spawn,
    /// The particle arrives from the origin `from`, having made a physical move: gravity's impulse
    /// is applied to its velocity. The cell's verdict names that origin.
    Receive { from: Dir, origin: State },
    /// The cell accepts a request from the requester toward `to`: its particle is committed to the
    /// transfer, in transport until the cell resolves it. The cell's verdict names that requester.
    Give { to: Dir },
    /// The particle the cell requested arrives from the origin `from` (as the origin published it,
    /// still holding it), having made a physical move: gravity's impulse is applied to its velocity.
    Take { origin: State },
    /// The particle stays, and its intent turns away from the target `from`, perpendicular to it, by
    /// its material's deflection: to the side it already leans or, pointing dead on, to the side
    /// `bits` picks.
    Deflect { from: Dir, bits: u32 },
}

impl Offer {
    /// The offer a cell has out, if any: its particle is committed to it, so the cell proposes
    /// nothing new until the offer resolves.
    pub fn of(me: State) -> Option<Offer> {
        me.offer.map(|toward| Offer { toward })
    }

    /// The verdict, as the origin reads it from the target's published state: accepted if the
    /// target's acceptance names this cell, rejected if its rejections do, and none yet otherwise.
    pub fn verdict(self, target: State) -> Option<Verdict> {
        let me = self.toward.opposite();
        if target.accept == Some(me) {
            Some(Verdict::Accepted)
        } else if target.rejected(me) {
            Some(Verdict::Rejected)
        } else {
            None
        }
    }

    /// The terms: the effects on (origin, target) under each condition. `origin` is the origin's
    /// published state, which the target reads; `bits` are the origin's random bits.
    pub fn terms(self, verdict: Verdict, origin: State, bits: u32) -> (Effect, Effect) {
        match verdict {
            Verdict::Accepted => {
                let released = if origin.material.is_substantial() { Effect::Release } else { Effect::Spawn };
                (released, Effect::Receive { from: self.toward.opposite(), origin })
            }
            Verdict::Rejected => (Effect::Deflect { from: self.toward, bits }, Effect::None),
        }
    }
}

impl Request {
    /// The request a cell has out, if any: its capacity is tied up in it until it resolves.
    pub fn of(me: State) -> Option<Request> {
        me.request.map(|from| Request { from })
    }

    /// The verdict, as the requester reads it from the origin's published state: accepted if the
    /// origin's acceptance names this cell, rejected if its rejections do, and none yet otherwise.
    pub fn verdict(self, origin: State) -> Option<Verdict> {
        let me = self.from.opposite();
        if origin.accept == Some(me) && origin.gives {
            Some(Verdict::Accepted)
        } else if origin.rejected(me) {
            Some(Verdict::Rejected)
        } else {
            None
        }
    }

    /// The terms: the effects on (origin, target) under each condition. `origin` is the origin's
    /// published state. The origin's `Give` it applies when it evaluates; the requester's `Take`
    /// when it resolves.
    pub fn terms(self, verdict: Verdict, origin: State) -> (Effect, Effect) {
        match verdict {
            Verdict::Accepted => (Effect::Give { to: self.from.opposite() }, Effect::Take { origin }),
            Verdict::Rejected => (Effect::None, Effect::None),
        }
    }
}

impl Effect {
    /// Apply this effect to the cell's own live state.
    pub fn apply(self, live: &mut State, physics: &Physics) {
        match self {
            Effect::None => {}
            Effect::Release => *live = State::VACUUM,
            Effect::Spawn => *live = State::holding(live.material),
            Effect::Receive { from, origin } => {
                live.material = origin.material;
                live.velocity = physics.moved(origin.material, origin.velocity);
                live.accept = Some(from);
            }
            Effect::Give { to } => {
                live.accept = Some(to);
                live.gives = true;
            }
            Effect::Take { origin } => {
                live.material = origin.material;
                live.velocity = physics.moved(origin.material, origin.velocity);
            }
            Effect::Deflect { from, bits } => {
                let (ix, iy) = deflection(physics.deflection(live.material) as i32, live.velocity, from, bits);
                live.velocity = physics.impelled(live.velocity, ix, iy);
            }
        }
    }
}

/// The deflection impulse on `v`, rejected toward `d`: `k` perpendicular to `d`, to the side it
/// already leans (or, dead on, the side `bits` picks).
pub fn deflection(k: i32, v: Velocity, d: Dir, bits: u32) -> (i32, i32) {
    if k == 0 {
        return (0, 0);
    }
    let (sx, sy) = d.step();
    // The perpendicular (-sy, sx), and how far the velocity already leans along it.
    let lean = -(v.x as i32) * sy + v.y as i32 * sx;
    let side = if lean > 0 || (lean == 0 && bits & 1 == 1) { 1 } else { -1 };
    (-side * k * sy / 2, side * k * sx / 2)
}
