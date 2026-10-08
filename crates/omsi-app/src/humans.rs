//! People on foot: passengers and pedestrians as agents with a goal.
//!
//! The simulation is omsi-sim's `people` (`PeopleSim`, which knows nothing of the GPU);
//! here it gets the loaded world, the traffic and the player's duty, and the people's
//! pictures (`crate::view_sync::people`): their meshes, posing and skinning, and the coins
//! on the cash desk.

use crate::ambience;
use crate::money::TicketBlocks;
use crate::scene::World;
use crate::traffic::Traffic;
use glam::DVec3;
use hashbrown::HashMap;
use omsi_render::{Camera, Renderer, Scene};
use omsi_sim::human::HumanType;
use omsi_sim::people::pax::{StopPlan, TripPlan};
use omsi_sim::people::{DutyTrip, PeopleSim};
use omsi_sim::VehicleInstance;
use parking_lot::MutexGuard;
use std::path::Path;
use std::sync::Arc;

// The renderer's side of the people.
use crate::view_sync::people::Bodies;

// The module's API, at the paths it always had (some only returned, never named outside).
#[allow(unused_imports)]
pub use omsi_sim::people::{AvatarCmd, SeatSpot};
#[allow(unused_imports)]
pub use omsi_sim::people::{placed_bus_id, remote_bus_id, remote_bus_player, LanPerson, MirrorPose};
pub use omsi_sim::people::{BusId, DoorWants};
#[allow(unused_imports)]
pub use omsi_sim::people::Person;
#[allow(unused_imports)]
pub use omsi_sim::people::VoiceLine;

/// The map's traffic keeps left (its stops are on the left): see the doors of `Cabin`.
pub(crate) use omsi_sim::people::LEFT_HAND;

/// The people with what the game makes of them: their pictures. Everything of the
/// simulation reads through it (`Deref` to `PeopleSim`).
pub struct Humans {
    pub sim: PeopleSim,
    /// The renderer's side: meshes, GPU materials, posing (see `crate::view_sync::people`;
    /// kept here because every call that makes or removes people shows them before it
    /// returns).
    pub(crate) view: Bodies,
    /// The tear-off ticket blocks of the player's bus (`money::TicketBlocks`).
    pub ticket_blocks: Option<TicketBlocks>,
}

impl std::ops::Deref for Humans {
    type Target = PeopleSim;
    fn deref(&self) -> &PeopleSim {
        &self.sim
    }
}

impl std::ops::DerefMut for Humans {
    fn deref_mut(&mut self) -> &mut PeopleSim {
        &mut self.sim
    }
}

/// Where the player looks from (`omsi_sim::people::Eye`), made from the camera.
pub struct Eye;

impl Eye {
    pub fn of(cam: &Camera, aspect: f32) -> omsi_sim::people::Eye {
        omsi_sim::people::Eye::looking(cam.position, cam.forward().as_dvec3(), cam.fov_deg as f64, aspect)
    }
}

/// What the people need of the loaded world (see `omsi_sim::people::World`).
impl omsi_sim::people::World for World {
    fn root(&self) -> &Path {
        &self.root
    }
    fn map_dir(&self) -> &Path {
        &self.map_dir
    }
    fn walk_height(&self, x: f64, y: f64) -> Option<f64> {
        World::walk_height(self, x, y)
    }
    fn walk_height_near(&self, x: f64, y: f64, near: f64) -> Option<f64> {
        World::walk_height_near(self, x, y, near)
    }
    fn has_ground(&self, x: f64, y: f64) -> bool {
        World::has_ground(self, x, y)
    }
    fn stop_exit_weight(&self, id: i64) -> f32 {
        World::stop_exit_weight(self, id)
    }
    fn stop_enter(&self, id: i64) -> (f32, f32) {
        World::stop_enter(self, id)
    }
    fn stop_side(&self, id: i64) -> f32 {
        World::stop_side(self, id)
    }
    fn stop_length(&self, id: i64) -> f32 {
        World::stop_length(self, id)
    }
    fn bus_stops(&self) -> MutexGuard<'_, Vec<(i64, DVec3, f64, String)>> {
        self.bus_stops.lock()
    }
    fn waiting_places(&self) -> MutexGuard<'_, Vec<(i64, DVec3, f64, f32)>> {
        self.waiting_places.lock()
    }
    fn object_positions(&self) -> MutexGuard<'_, HashMap<i64, (DVec3, [f64; 3])>> {
        self.object_positions.lock()
    }
    fn collision(&self) -> MutexGuard<'_, Arc<omsi_sim::collision::CollisionWorld>> {
        self.collision.lock()
    }
    fn parked_boxes(&self) -> MutexGuard<'_, Arc<Vec<omsi_sim::collision::Obb>>> {
        self.parked_boxes.lock()
    }
    fn tiles_generation(&self) -> u64 {
        self.tiles_generation.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Humans {
    pub fn new(root: &Path) -> Humans {
        let configured_people = crate::settings::Settings::load().ai_max_humans as usize;
        Humans { sim: PeopleSim::new(root, configured_people), view: Bodies::new(), ticket_blocks: None }
    }

    /// Advance everybody (see `PeopleSim::tick_inner`), and show the renderer what the step
    /// did. `bus`: the player's vehicle; `traffic`: the timetable buses, the traffic lights
    /// and the cars pedestrians wait for. Returns true when a passenger took the printed
    /// ticket (the caller resets `GivenTicket`).
    pub fn tick(
        &mut self,
        dt: f32,
        world: &World,
        bus: Option<&VehicleInstance>,
        traffic: Option<&Traffic>,
        renderer: &Renderer,
        scene: &mut Scene,
    ) -> bool {
        let started = std::time::Instant::now();
        self.sim.tick_stages.clear();
        let took = self.sim.tick_inner(dt, world, bus, traffic.map(|t| &t.sim));
        // what the step did, drawn (people who came and went, coins on the desk)
        let mark = std::time::Instant::now();
        self.show_bodies(world, renderer, scene);
        self.sim.tick_stages.push(("bodies", mark.elapsed().as_secs_f64() * 1000.0));
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        self.sim.tick_done(dt, world, ms);
        took
    }

    /// Put people at the bus stops near `center` (see `PeopleSim::populate`).
    pub fn populate(&mut self, world: &World, renderer: &Renderer, scene: &mut Scene, center: DVec3) {
        self.sim.populate(world, center);
        self.show_bodies(world, renderer, scene);
    }

    /// Put avatar `key` where `cmd` says (see `PeopleSim::avatar`).
    pub fn avatar(&mut self, key: u32, world: &World, renderer: &Renderer, scene: &mut Scene, cmd: AvatarCmd, kind: u64) {
        self.sim.avatar(key, world, cmd, kind);
        self.show_bodies(world, renderer, scene);
    }

    /// One of the host's people appears here (client; see `PeopleSim::mirror_add`).
    pub fn mirror_add(&mut self, world: &World, renderer: &Renderer, scene: &mut Scene, id: u32, ty: usize, pose: &MirrorPose) -> bool {
        let added = self.sim.mirror_add(world, id, ty, pose);
        self.show_bodies(world, renderer, scene);
        added
    }

    /// `--riders n` (see `PeopleSim::seed_riders`).
    pub fn seed_riders(&mut self, n: usize, bus: &VehicleInstance, world: &World, renderer: &Renderer, scene: &mut Scene) {
        self.sim.seed_riders(n, bus, world);
        self.show_bodies(world, renderer, scene);
    }

    /// OMSI's `change_take`: the driver takes back the coins lying on the change tray.
    pub fn take_change_tray(&mut self) {
        if let Some(m) = self.sim.money.as_mut() {
            m.clear(true);
        }
    }

    /// Bus `bus` is gone (the player removed it; see `PeopleSim::evict`).
    pub fn evict(&mut self, bus: BusId, world: &World) {
        self.sim.evict(bus, world);
    }

    /// The stop the player's duty is due at next (see `PeopleSim::set_player_next_stop`).
    pub fn set_player_next_stop(&mut self, stop: Option<&crate::schedule::PlannedStop>) {
        self.sim.set_player_next_stop(stop.map(|s| (s.object_id, s.name.as_str(), s.position)));
    }

    /// The player's duty this frame; None in free drive, where the bus takes whom its terminus
    /// shown takes, as a timetable bus. (Set after `stop_names`: the trip's stops are named by it.)
    pub fn set_duty(&mut self, duty: Option<&crate::schedule::PlayerDuty>) {
        let Some(d) = duty else {
            self.sim.duty = None;
            return;
        };
        // the trip the IBIS is given: on a works trip from the depot the next one with a line
        let (t, next) = d.trip_for_ibis();
        let done = std::ptr::eq(t, d.trip()) && d.trip_done();
        let trip = match self.sim.duty.take() {
            Some((trip, ..)) if trip.name == t.name && trip.departure == t.departure => trip,
            _ => Arc::new(DutyTrip::of(&trip_plan(t), self.sim.stop_names.as_ref())),
        };
        self.sim.duty = Some((trip, next, done));
    }

    /// The footsteps taken since the last call, for the environment sounds (see
    /// `PeopleSim::take_footfalls`).
    pub fn take_footfalls(&mut self) -> Vec<ambience::Footfall> {
        self.sim
            .take_footfalls()
            .into_iter()
            .map(|f| ambience::Footfall { position: f.position, inside: f.inside, own_bus: f.own_bus, pack: f.pack })
            .collect()
    }

    /// `OMSI_TRACE_PAX` is writing a trace.
    pub fn tracing(&self) -> bool {
        self.view.trace.is_some()
    }

    /// Count of people per state, for logs, and the time posing them took.
    pub fn summary(&self) -> String {
        let mut out = self.sim.summary();
        let (frames, posed, ms, up) = self.view.pose_stats;
        if frames > 0 {
            out.push_str(&format!(
                "; posing {:.2} ms a frame ({:.1} people, {:.2} ms of it uploading and placing)",
                ms / frames as f64,
                posed as f64 / frames as f64,
                up / frames as f64
            ));
        }
        out
    }

    /// The file of a human type relative to its content root (`Humans/…/x.hum`).
    pub fn type_file(ty: &HumanType) -> String {
        PeopleSim::type_file(ty)
    }

    /// Hand a bus's scripts what the passengers want of its doors and places (see
    /// `PeopleSim::write_door_requests`).
    pub(crate) fn write_door_requests(b: &mut VehicleInstance, doors: &DoorWants) {
        PeopleSim::write_door_requests(b, doors)
    }
}

/// A trip of the timetable as the passengers need it.
fn trip_plan(trip: &crate::schedule::PlannedTrip) -> TripPlan {
    TripPlan {
        name: trip.name.clone(),
        line: trip.line.clone(),
        terminus: trip.terminus.clone(),
        departure: trip.departure,
        stops: trip.stops.iter().map(|s| StopPlan { object_id: s.object_id, name: s.name.clone(), stops: s.stops }).collect(),
    }
}
