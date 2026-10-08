//! View sync: the one place that turns the simulation's state into renderer instances (see
//! `docs/ARCHITECTURE.md`, "Target layering"). The AI traffic and the people simulate in
//! omsi-sim and know nothing of the GPU; what they look like is here:
//!
//! * `traffic`: the AI vehicles' renders, their drivers and the traffic light lamps;
//! * `people`: the people's meshes, posing and skinning, the coins and the ticket blocks.
//!
//! [`sync`] is the frame's view sync. The window runs it twice a frame, each time where the
//! part it brings up to date used to be synced: the traffic at the end of its step (before
//! the player and the people move: parking cars leave the traffic's list there, which the
//! people's step reads), the people at the end of theirs. The offscreen run calls it at the
//! same points, and before each picture it takes.
//!
//! Two kinds of renderer change still happen inside the simulation wrappers' calls, at the
//! moment the simulation makes them, so that the renderer sees the same calls in the same
//! order: a car's renders are made or let go as the traffic puts it on the road or takes it
//! off (`Traffic::view`), and the people who appeared or went are shown at the end of every
//! `Humans` call that made them (`Humans::show_bodies`). Their state lives in the wrappers
//! (`Traffic::view`, `Humans::view`) for that reason; its types and code are here.

pub(crate) mod people;
pub(crate) mod traffic;

use crate::humans::Humans;
use crate::scene::World;
use crate::traffic::Traffic;
use glam::DVec3;
use omsi_render::{Renderer, Scene};
use omsi_sim::VehicleInstance;

/// The people to bring up to date in a view sync.
pub(crate) struct People<'a> {
    pub(crate) humans: &'a mut Humans,
    /// The player's bus: its coins on the cash desk and its ticket blocks (None: no bus, or
    /// those are not wanted at this point).
    pub(crate) bus: Option<&'a VehicleInstance>,
    /// Where the people are seen from when they have no eye (`PeopleSim::eye`): the nearer,
    /// the more often one is posed.
    pub(crate) camera: DVec3,
}

/// What a view sync brings up to date: the traffic, the people, or both (in that order).
#[derive(Default)]
pub(crate) struct ViewSync<'a> {
    pub(crate) traffic: Option<&'a mut Traffic>,
    pub(crate) people: Option<People<'a>>,
}

impl<'a> ViewSync<'a> {
    /// The traffic alone.
    pub(crate) fn traffic(traffic: &'a mut Traffic) -> ViewSync<'a> {
        ViewSync { traffic: Some(traffic), people: None }
    }

    /// The people alone (see [`People`]).
    pub(crate) fn people(humans: &'a mut Humans, bus: Option<&'a VehicleInstance>, camera: DVec3) -> ViewSync<'a> {
        ViewSync { traffic: None, people: Some(People { humans, bus, camera }) }
    }
}

/// Bring the renderer up to the simulation's state: the traffic (cars that have parked
/// become parked objects, renders of cars gone go back to the world, the drivers, the
/// traffic lamps, the cars' transforms, materials and script textures), then the people
/// (the coins and ticket blocks of the player's bus, those who went since, the posing and
/// skinning and their transforms).
pub(crate) fn sync(what: ViewSync<'_>, world: &World, renderer: &Renderer, scene: &mut Scene) {
    if let Some(t) = what.traffic {
        t.sync(world, renderer, scene);
    }
    if let Some(People { humans, bus, camera }) = what.people {
        if let Some(bus) = bus {
            humans.sync_money(world, renderer, scene, bus);
        }
        humans.sync(renderer, scene, camera);
    }
}
