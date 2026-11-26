//! A cell's update, stage by stage: read, resolve, evaluate, evolve, publish, propose.
//!
//! Each stage works on the cell's live state and reads only its neighbors' published state. The
//! contracts the stages execute are in `contract`; the selections in evaluate and propose are
//! biased by the physics distribution (`physics`).

use crate::contract::{Effect, Offer, Request, Verdict};
use crate::selection::select;
use crate::{Dir, Physics, State, hash};

/// Salts for the selections, so they draw independent random bits.
pub const SALT_EVALUATE: u32 = 1;
pub const SALT_PROPOSE: u32 = 2;
pub const SALT_DEFLECT: u32 = 3;
pub const SALT_WAIT: u32 = 4;
pub const SALT_REQUEST: u32 = 5;

/// One cell's update: its next published state from its own state, the state it kept back from
/// the phase before, and its neighbors' published state, all at the same phase (outside the
/// lattice reads as bedrock). The cell computes only itself.
#[allow(clippy::too_many_arguments)]
pub fn next(me: State, kept: State, seen: [State; 6], x: i32, y: i32, phase: u32, physics: &Physics) -> State {
    // Read: `seen` is the neighbors' published state; the live state starts as my own, its
    // contracts cleared until the stages below write them again.
    let mut live = State { offer: None, accept: None, reject: 0, request: None, gives: false, ..me };
    // An offer of mine being evaluated now, or one with no verdict yet (it stands), ties up my
    // particle, and with it my capacity; a request of mine ties up my capacity. Either way I
    // propose nothing new, and evaluate rejects every contract but a matching one.
    let standing = resolve(&mut live, me, kept, &seen, x, y, phase, physics);
    let committed = me.offer.is_some() || me.request.is_some() || standing;

    evaluate(&mut live, me, committed, &seen, x, y, phase, physics);
    evolve(&mut live);
    // Publish, then propose. They share one write: the word a cell writes at the end of its update
    // holds both its published state and the contracts it just proposed, and neighbors read them
    // together at the next phase, as if published first.
    // On initialization, a cell that waits (a coin flip, `World::new`) lets its first phase pass.
    // A particle given to a requester is committed to it.
    live.waits = false;
    if !committed && !me.waits && !live.gives {
        propose(&mut live, &seen, x, y, phase, physics);
    }
    live
}

/// Resolve: settle my contracts that have verdicts.
/// - A particle I gave to a requester (my `gives`) leaves: I hold nothing, or for air, spawn a new
///   one.
/// - The offer I published last phase (now in my kept-back state) has its verdict, if any, in the
///   target's published state. With one, I apply its terms' effect on me, the origin.
/// - The same for a request, whose verdict is in the origin's published state; a request matched by
///   the origin's offer, which I accepted, is settled already.
///
/// A contract with no verdict yet stands: I publish it again. Returns whether one stands.
#[allow(clippy::too_many_arguments)]
fn resolve(
    live: &mut State,
    me: State,
    kept: State,
    seen: &[State; 6],
    x: i32,
    y: i32,
    phase: u32,
    physics: &Physics,
) -> bool {
    if me.gives {
        let released = if me.material.is_substantial() { Effect::Release } else { Effect::Spawn };
        released.apply(live, physics);
    }
    if let Some(offer) = Offer::of(kept) {
        let Some(verdict) = offer.verdict(seen[offer.toward as usize]) else {
            live.offer = kept.offer;
            return true;
        };
        let (mine, _) = offer.terms(verdict, kept, hash(x, y, phase, SALT_DEFLECT));
        mine.apply(live, physics);
    }
    if let Some(request) = Request::of(kept) {
        if me.accept == Some(request.from) {
            return false;
        }
        let origin = seen[request.from as usize];
        let Some(verdict) = request.verdict(origin) else {
            live.request = kept.request;
            return true;
        };
        let (_, mine) = request.terms(verdict, origin);
        mine.apply(live, physics);
    }
    false
}

/// Evaluate: the contracts that target me, selected one at a time, biased by the physics
/// distribution. Offers into me it doesn't entertain are excluded, and the rest weighted (heavier
/// particles and better-aligned moves first); so are requests for my particle, which I can give
/// only while it isn't tied up, weighted by how well my particle's move toward the requester lines
/// up. The selected contract is accepted, and I apply its terms' effect on me: for an offer, I
/// receive the particle; for a request, I give mine. While contracts remain I select again, but a
/// cell holds one particle at most: once one is accepted, the rest are excluded. Every contract I
/// evaluate gets a verdict: the accepted one is named in `accept`, every other in `reject`.
///
/// A matched pair is accepted automatically: an offer from the neighbor I'm requesting from, which
/// I accept, and a request from the neighbor I'm offering to, which gets no verdict from me (its
/// author accepts my offer).
#[allow(clippy::too_many_arguments)]
fn evaluate(
    live: &mut State,
    me: State,
    committed: bool,
    seen: &[State; 6],
    x: i32,
    y: i32,
    phase: u32,
    physics: &Physics,
) {
    let matched = me.request.filter(|&d| {
        let origin = seen[d as usize];
        origin.offer == Some(d.opposite()) && physics.entertains(origin.material, live.material, false)
    });
    let mut weights = [0; 6];
    let mut asked = 0u8;
    for d in Dir::ALL {
        let other = seen[d as usize];
        if other.offer == Some(d.opposite()) {
            asked |= 1 << d as u8;
            if matched.is_none() && physics.entertains(other.material, live.material, committed) {
                weights[d as usize] =
                    physics.weight(other.material, other.velocity, d.opposite(), live.material).max(1);
            }
        } else if other.request == Some(d.opposite()) && me.offer != Some(d) {
            asked |= 1 << d as u8;
            if matched.is_none() && !committed && physics.entertains(live.material, other.material, false) {
                weights[d as usize] = physics.weight(live.material, live.velocity, d, other.material).max(1);
            }
        }
    }
    let accepted = matched.map(|d| d as usize).or_else(|| select(&weights, hash(x, y, phase, SALT_EVALUATE)));
    live.reject = asked & !accepted.map_or(0, |i| 1 << i);
    let Some(i) = accepted else { return };
    let (d, other) = (Dir::ALL[i], seen[i]);
    let mine = if other.offer == Some(d.opposite()) {
        Offer { toward: d.opposite() }.terms(Verdict::Accepted, other, 0).1
    } else {
        Request { from: d.opposite() }.terms(Verdict::Accepted, *live).0
    };
    mine.apply(live, physics);
}

/// Evolve: natural contracts, within the cell. None yet.
fn evolve(_live: &mut State) {}

/// Propose: one phase in `request_odds`, chosen at random, I propose a request; otherwise an offer.
///
/// An offer: the particle I host offers to move toward a neighbor, following its velocity. Start
/// with all six neighbors, exclude the incompatible (`Physics::offers_to`), and select among the
/// rest, weighted. Air with vacuum in its neighborhood offers into the vacuum
/// (`Physics::air_meets_vacuum`). Offering commits the particle.
///
/// A request: I ask a neighbor to transfer its particle to me. Exclude the neighbors whose particle
/// I have no room for (`Physics::requests_from`), and select among the rest by how strongly each
/// particle's velocity points at me. Requesting ties up my capacity.
fn propose(live: &mut State, seen: &[State; 6], x: i32, y: i32, phase: u32, physics: &Physics) {
    let mut weights = [0; 6];
    if physics.requests(hash(x, y, phase, SALT_REQUEST)) {
        for d in Dir::ALL {
            let other = seen[d as usize];
            if physics.requests_from(other.material, live.material) {
                weights[d as usize] = physics.preference(other.material, other.velocity, d.opposite());
            }
        }
        live.request = select(&weights, hash(x, y, phase, SALT_PROPOSE)).map(|i| Dir::ALL[i]);
        return;
    }
    let only_vacuum = physics.air_meets_vacuum(live.material, seen);
    for d in Dir::ALL {
        let target = seen[d as usize].material;
        if only_vacuum && target != crate::Material::Vacuum {
            continue;
        }
        if physics.offers_to(live.material, target) {
            weights[d as usize] = physics.weight(live.material, live.velocity, d, target);
        }
    }
    live.offer = select(&weights, hash(x, y, phase, SALT_PROPOSE)).map(|i| Dir::ALL[i]);
}
