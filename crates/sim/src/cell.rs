//! A cell: a position on the lattice holding all in-place state. It keeps its phase (see `world`,
//! the synchronizer), its live particle and its live contracts, and publishes them for its
//! neighbors to read.

use crate::{Dir, Material, Velocity};

/// What a cell publishes at a phase: its live particle (a material and a velocity; vacuum when it
/// holds nothing) and its live contracts (see `contract` for how each is read).
///
/// - `offer`: an offer this cell authored, as origin, to the neighbor toward that direction. Its
///   particle is committed to it until the cell resolves it.
/// - `accept`: this cell's verdict, as target, on the offers it evaluated: it names the origin whose
///   offer it accepted. Every other offer that targeted it then was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct State {
    pub material: Material,
    pub velocity: Velocity,
    pub offer: Option<Dir>,
    pub accept: Option<Dir>,
}

impl State {
    pub const AIR: State = State::holding(Material::Air);
    pub const BEDROCK: State = State::holding(Material::Bedrock);
    pub const VACUUM: State = State::holding(Material::Vacuum);

    /// A cell holding a fresh particle of `material` (or nothing, for vacuum), with no contracts.
    /// A fresh particle that moves intends to move down.
    pub const fn holding(material: Material) -> State {
        let velocity = if material.moves() { Velocity::DOWN } else { Velocity::ZERO };
        State { material, velocity, offer: None, accept: None }
    }

    /// Bits 0-2 the material, 3-5 the offer and 6-8 the acceptance (1 + a direction, or 0 for
    /// none). The velocity has a word of its own (`Velocity::encode`).
    pub fn encode(self) -> u32 {
        let dir = |d: Option<Dir>| d.map_or(0, |d| d as u32 + 1);
        self.material as u32 | dir(self.offer) << 3 | dir(self.accept) << 6
    }

    pub fn decode(bits: u32, velocity: u32) -> State {
        let dir = |b: u32| match b & 7 {
            0 | 7 => None,
            n => Some(Dir::ALL[n as usize - 1]),
        };
        State {
            material: Material::decode(bits & 7),
            velocity: Velocity::decode(velocity),
            offer: dir(bits >> 3),
            accept: dir(bits >> 6),
        }
    }

    /// What the cell counts as holding in a census of published state: a particle accepted this
    /// phase is still in transport (its origin counts it until it resolves), so the cell counts
    /// as what it held before: vacuum.
    pub fn counts_as(self) -> Material {
        if self.accept.is_some() { Material::Vacuum } else { self.material }
    }
}
