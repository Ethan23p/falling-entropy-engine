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
pub const SALT_HOLD: u32 = 6;

/// One cell's update: its next published state from its own state, the state it kept back from
/// the phase before, and its neighbors' published state, all at the same phase (outside the
/// lattice reads as bedrock). The cell computes only itself.
#[allow(clippy::too_many_arguments)]
pub fn next(me: State, kept: State, seen: [State; 6], x: i32, y: i32, phase: u32, physics: &Physics) -> State {
    // Read: `seen` is the neighbors' published state; the live state starts as my own, its
    // contracts cleared until the stages below write them again.
    let mut live =
        State { offer: None, accept: None, reject: 0, request: None, gives: false, wait: None, held: false, ..me };
    // An offer of mine being evaluated now, or one that stands (told to wait, or with no verdict
    // yet), ties up my particle, and with it my capacity; a request of mine ties up my capacity.
    // Either way I propose nothing new, and evaluate accepts no contract but a matching one: it
    // keeps one offer on the hook, for the capacity my contract may free, and rejects the rest. A
    // held offer of mine is resolved now, so it ties up nothing past resolve.
    let standing = resolve(&mut live, me, kept, &seen, x, y, phase, physics);
    let committed = (me.offer.is_some() && !me.held) || me.request.is_some() || standing;

    evaluate(&mut live, me, committed, &seen, x, y, phase, physics);
    evolve(&mut live);
    // Publish, then propose. They share one write: the word a cell writes at the end of its update
    // holds both its published state and the contracts it just proposed, and neighbors read them
    // together at the next phase, as if published first.
    // On initialization, a cell that delays (a coin flip, `World::new`) lets its first phase pass.
    // A particle given to a requester is committed to it.
    live.delays = false;
    if !committed && !me.delays && !live.gives {
        propose(&mut live, &seen, x, y, phase, physics);
    }
    live
}

/// Resolve: settle my contracts that have verdicts.
/// - A particle I gave to a requester (my `gives`) leaves: I hold nothing, or for air, spawn a new
///   one.
/// - The offer I published last phase (now in my kept-back state) has its verdict, if any, in the
///   target's published state. With one, I apply its terms' effect on me, the origin. Told to wait,
///   I hold it: it stands one more phase, `held`. A held offer (in my own published state) has its
///   final verdict now.
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
    // A held offer is resolved from my own published state, the phase it is published; the phase
    // after, it is in my kept-back state, resolved already.
    let (offer, origin) = if me.held { (Offer::of(me), me) } else { (Offer::of(kept).filter(|_| !kept.held), kept) };
    if let Some(offer) = offer {
        let Some(verdict) = offer.verdict(seen[offer.toward as usize]) else {
            live.offer = Some(offer.toward);
            return true;
        };
        let (mine, _) = offer.terms(verdict, origin, hash(x, y, phase, SALT_DEFLECT));
        mine.apply(live, physics);
        if verdict == Verdict::Wait {
            return true;
        }
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
/// receive the particle; for a request, I give mine. Either ties up what it deals in: a received
/// particle is in transport, and fills my capacity; a given one is committed to its requester. While
/// contracts remain I select again, from those my resources still entertain. Every contract I
/// evaluate gets a verdict: an accepted one is named in `accept`, a waited one in `wait`, every
/// other in `reject`.
///
/// A matched pair is accepted automatically: an offer from the neighbor I'm requesting from, which
/// I accept first, and a request from the neighbor I'm offering to, which gets no verdict from me
/// (its author accepts my offer).
///
/// Wait: while my resources are committed to a contract of my own, I accept nothing (but a matched
/// pair), and keep one offer on the hook instead: selected, weighted as above, from the offers the
/// capacity my contract may free would entertain (`freed`). The rest I reject. Next phase, the
/// offer I kept is settled first, accepted if my resources entertain it then, rejected if not; its
/// origin still holds the particle, committed, and publishes the offer `held`, which isn't a new
/// offer to evaluate.
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
    let offers = |d: Dir| seen[d as usize].offer == Some(d.opposite()) && !seen[d as usize].held;
    let mine_out = me.offer.filter(|_| !me.held);
    let asked = Dir::ALL.iter().fold(0u8, |bits, &d| {
        let other = seen[d as usize];
        let requests = other.request == Some(d.opposite()) && mine_out != Some(d);
        bits | ((offers(d) || requests || me.wait == Some(d)) as u8) << d as u8
    });
    let mut tied = committed;
    let mut taken = 0u8;
    // The offer I kept on the hook, settled first: accepted only if my resources entertain it now.
    let waited = me.wait.filter(|&d| physics.entertains(seen[d as usize].material, live.material, tied));
    let matched =
        me.request.filter(|&d| offers(d) && physics.entertains(seen[d as usize].material, live.material, false));
    let mut next = waited.or(matched);
    for round in 0..6 {
        let d = match next.take() {
            Some(d) => d,
            None => {
                let mut weights = [0; 6];
                for d in Dir::ALL {
                    let other = seen[d as usize];
                    // The offer I kept on the hook is settled above, or rejected.
                    if (asked & !taken) >> d as u8 & 1 == 0 || me.wait == Some(d) {
                        continue;
                    }
                    if offers(d) {
                        if physics.entertains(other.material, live.material, tied) {
                            weights[d as usize] =
                                physics.weight(other.material, other.velocity, d.opposite(), live.material).max(1);
                        }
                    } else if !tied && physics.entertains(live.material, other.material, false) {
                        weights[d as usize] = physics.weight(live.material, live.velocity, d, other.material).max(1);
                    }
                }
                match select(&weights, hash(x, y, phase, SALT_EVALUATE | round << 8)) {
                    Some(i) => Dir::ALL[i],
                    None => break,
                }
            }
        };
        taken |= 1 << d as u8;
        let other = seen[d as usize];
        let mine = if me.wait == Some(d) || offers(d) {
            Offer { toward: d.opposite() }.terms(Verdict::Accepted, other, 0).1
        } else {
            Request { from: d.opposite() }.terms(Verdict::Accepted, *live).0
        };
        mine.apply(live, physics);
        tied = true;
    }
    if committed && live.accept.is_none() {
        let freed = freed(live, me);
        let mut weights = [0; 6];
        for d in Dir::ALL {
            let other = seen[d as usize];
            if (asked & !taken) >> d as u8 & 1 == 1 && offers(d) && physics.entertains(other.material, freed, false) {
                weights[d as usize] = physics.weight(other.material, other.velocity, d.opposite(), freed).max(1);
            }
        }
        live.wait = select(&weights, hash(x, y, phase, SALT_HOLD)).map(|i| Dir::ALL[i]);
        if let Some(d) = live.wait {
            taken |= 1 << d as u8;
        }
    }
    live.reject = asked & !taken;
}

/// What my capacity holds once the contract my resources are committed to settles, if it frees:
/// an offered particle leaves (air spawns a new air, which still takes a substantial particle), and
/// a request's reserved capacity is free again if it's rejected.
fn freed(live: &State, me: State) -> crate::Material {
    let offering = live.offer.is_some() || (me.offer.is_some() && !me.held);
    if offering && live.material.is_substantial() { crate::Material::Vacuum } else { live.material }
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
