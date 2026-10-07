//! AI road traffic: vehicles from `ailists.cfg` moving on the map's path network, the
//! traffic light programs of the junctions, and the population of cars around the player.
//!
//! The simulation (`model`, `tick`, `planning`, `obstacles`, `junctions`, `lights`,
//! `dormant`, `density`) knows nothing of the GPU; what the cars look like on the screen is
//! `view`'s.

mod audio;
mod control;
mod density;
mod dormant;
mod junctions;
mod lights;
mod mirror;
mod model;
mod obstacles;
mod parked;
mod planning;
mod population;
mod setup;
mod tick;
mod trains;
mod view;
mod viewer;
#[cfg(test)]
mod tests;

use crate::bus_service::{BusService, Phase};
use crate::scene::VehicleRender;
use anyhow::Result;
use glam::{DVec2, DVec3};
use hashbrown::HashMap;
use omsi_sim::ai_motion::{
    back_in_ramp, pull_out_ramps, AiBody, MotionKind, BACK_IN_LAT_ACCEL, PULL_OUT_ACCEL,
    PULL_OUT_CLEARANCE,
};
use omsi_sim::collision::Obb;
use omsi_sim::traffic::{
    arrival_time, AiState, Aspect, LaneKind, Lead, Network, TrafficLightController, MAX_BRAKE,
};
use omsi_sim::vehicle::AiFrame;
use omsi_sim::{VehicleInstance, VehicleType};
use std::path::Path;
use std::sync::Arc;

use density::*;
use dormant::*;
use junctions::*;
use lights::*;
use model::*;
use obstacles::*;
use parked::*;
use population::*;
use tick::*;
use viewer::*;

pub(crate) use model::{ibis_to_next_stop, vehicle_bodies, AiCar, BusSetup, DormantCar, ParkPlan, Passing, PlayerBox};
pub(crate) use setup::warm_up;
pub(crate) use viewer::Viewer;

/// How many of a type's paint schemes the AI uses: every scheme is a full upload of the
/// bus's textures the first time it appears, which used to cost a frame of 100-200 ms
/// each and a minute of stutter after loading Spandau.
pub const AI_SCHEMES: usize = 4;

pub struct Traffic {
    pub net: Network,
    /// Sum of the spawn weights of every street lane, updated only as tiles add lanes.
    street_weight: f64,
    /// Parked cars standing in or beside a lane: per lane, (distance along it, signed
    /// lateral offset of the car's centre, + = right). A car in the lane's middle is an
    /// obstacle to stop behind; one over the kerb side is passed with a swerve to the left.
    parked: HashMap<usize, Vec<(f32, f32)>>,
    /// Parked cars no lane has been found beside yet: the lane may come with a tile that is
    /// not loaded yet (a road spline often starts in the next tile). Most stand in car parks
    /// and stay here.
    parked_waiting: Vec<DVec3>,
    /// Tiles whose lanes the network has (lanes stay once they are in).
    pub lane_tiles: hashbrown::HashSet<(i32, i32)>,
    /// Counts the times tiles brought their lanes: whoever resolved something against the
    /// network and missed a part of it looks again when this changes.
    pub lanes_generation: u64,
    /// AI vehicle types with weight, the lane kind they run on (`[type]` 2 rail, 3 air)
    /// and their group in `groups`.
    types: Vec<(Arc<VehicleType>, f32, LaneKind, usize)>,
    /// The random traffic groups: name, `unsched_trafficdens.txt` factor and day curves.
    groups: Vec<omsi_map::ailists::UnschedGroup>,
    /// The map has an `unsched_trafficdens.txt` (else the global.cfg curve applies).
    group_curves: bool,
    /// Each group's place in `unsched_vehgroups.txt`, the number a path's `[rule]
    /// trafficdensity` names it by (None: the map has no such file, and every group drives
    /// wherever the lane's density lets traffic).
    group_uvg: Vec<Option<usize>>,
    /// The default density of every `unsched_vehgroups.txt` entry, in file order: 0 none, 1
    /// for the first entry its medium density, for any other that of the first entry, 2 of
    /// the second, and so on. It applies on the paths without a rule for the group.
    uvg_defaults: Arc<Vec<i32>>,
    pub cars: Vec<AiCar>,
    /// The random cars out of range (see `DormantCar`).
    pub dormant: Vec<DormantCar>,
    /// `time` when the dormant cars last moved on.
    dormant_time: f32,
    rng: u64,
    /// Target number of cars around the camera.
    pub target: usize,
    /// Made only so that the light programs run (no traffic, no timetable): nobody is put
    /// on the roads - no aircraft, no parked car pulling out - while `target` is 0.
    pub lights_only: bool,
    pub spawn_radius: f64,
    pub time: f32,
    /// Renders of cars that have gone, given back at the next `sync`.
    released: Vec<VehicleRender>,
    /// Where the camera is (the window sets it before `sync`): far cars show their script
    /// textures as stand-ins.
    pub camera: Option<DVec3>,
    /// Sound sets of despawned cars, stopped at the next audio update.
    orphan_sounds: Vec<omsi_audio::SoundSet>,
    lights: Vec<TrafficLightController>,
    controller_of_object: HashMap<i64, usize>,
    /// Coupled vehicle types by file.
    trailer_types: HashMap<std::path::PathBuf, Option<Arc<VehicleType>>>,
    /// `[sound_ai]` configurations by file.
    sound_cfgs: HashMap<std::path::PathBuf, Option<Arc<omsi_vehicle::SoundCfg>>>,
    root: std::path::PathBuf,
    /// Car-frames spent waiting for a red light (statistics).
    pub held_at_red: usize,
    /// Who wants a timetable bus to stop (`Humans::stop_wishes`): the buses somebody
    /// aboard wants to get off, the stops where somebody waits. None without passengers:
    /// every bus then serves every stop.
    stop_wishes: Option<(hashbrown::HashSet<u64>, hashbrown::HashSet<i64>)>,
    /// Seconds the player's vehicle has been standing.
    player_still: f32,
    /// Time of day (seconds since midnight); light cycles and timetables run on it.
    pub day_time: f64,
    /// How fast the clock runs (the time speed): the timetable keeps to it.
    pub time_scale: f64,
    /// Day of the week (0 Monday … 6 Sunday) for the traffic density curves.
    pub weekday: i32,
    /// Street lights on → AI vehicles switch their lights on.
    pub night: bool,
    /// The light of the day, for the cars' `Envir_Brightness` (see `sync`).
    pub daylight: Option<omsi_sim::Daylight>,
    next_id: u64,
    /// The last car that started an overtake and when (for chase-camera debugging).
    pub last_overtaker: Option<(u64, f32)>,
    /// The first car that entered a turning lane and when (`--follow turn`).
    pub first_turner: Option<(u64, f32)>,
    /// The first car that stopped at a red light (`--follow red`), the first that gave way
    /// at a junction (`--follow yield`), the first that pulled out onto the other side of
    /// the road round an obstacle or squeezed past a bus at its stop (`--follow pass`).
    pub first_red: Option<(u64, f32)>,
    pub first_yield: Option<(u64, f32)>,
    pub first_passer: Option<(u64, f32)>,
    /// `[trafficdensity_road]` curve of the map: (hour, factor).
    pub density_curve: Vec<(f32, f32)>,
    /// The options' `[AIUnschedFactor]`: the share of the random traffic.
    pub unsched_factor: f32,
    /// The options' `[AIMaxCountScheduled]` (0 = no limit).
    pub max_scheduled: u32,
    /// `--no-timetable-buses`: the timetable runs for the player's duty, but puts no AI
    /// bus on the road (#1762).
    pub no_timetable_buses: bool,
    /// Where the player looks from (set every frame).
    pub viewer: Option<Viewer>,
    /// Buildings that hide what is behind them (the player's collision world).
    pub occluders: Option<Arc<omsi_sim::collision::CollisionWorld>>,
    /// Pedestrians on the footpaths: (lane, distance along it), for giving way at crossings
    /// and for the pedestrian lights' request buttons.
    pub walkers: Vec<(usize, f32)>,
    /// Everybody on foot on the ground: position, velocity and whether they are waiting
    /// at a stop (set every frame) - the cars stop for anybody in their way, not only on
    /// a crossing.
    pub people: Vec<(DVec3, DVec2, bool)>,
    /// No car has been placed yet: the first population may fill the view.
    initial: bool,
    /// Seconds of the last tick (the lamp scripts run in `sync`).
    last_dt: f32,
    /// Game time since the lamps' scripts last ran (see `sync`).
    lamp_dt: f32,
    /// `OMSI_TRACE_AI=<file.csv>`: every car's pose, steering and speed, every frame.
    trace: Option<std::io::BufWriter<std::fs::File>>,
    /// Cars already reported for a hard bend (`OMSI_DEBUG_TRAFFIC`).
    logged_hard: hashbrown::HashSet<u64>,
    /// `OMSI_DEBUG_LIGHTS=all|near|<controller>,…`: which programs log their changes, and
    /// the states they showed last.
    light_log: Option<String>,
    light_prev: Vec<Vec<i32>>,
    /// `OMSI_DEBUG_POPULATION`: log where cars appear and vanish relative to the view.
    debug_population: bool,
    /// Cars placed since the last look inside the view frustum (hidden behind something):
    /// (id, position). `OMSI_POPULATION_SHOTS` photographs them to check.
    pub framed_spawns: Vec<(u64, DVec3)>,
    /// The player's vehicle as of the last tick (nothing is put on the road on top of it).
    player: Option<PlayerBox>,
    /// The player's bus has right of way over the traffic (its script's `TrafficPriority`,
    /// OMSI: priority 1000 over the types' own): cars keep out of the way it is about
    /// to take for longer.
    pub player_priority: bool,
    /// The player's indicators (0 off, 1 left, 2 right, 3 hazard; `lan::indicator`), set
    /// before each `tick`.
    pub player_blinker: u8,
    /// Seconds since the player's bus last showed the indicator towards the traffic (the
    /// lamps go dark half of the time).
    player_signal_age: f32,
    /// ... and for how long it has been indicating so (s).
    player_signalling: f32,
    /// The player's vehicle and the LAN players' as the junctions see them (`way_user_on`),
    /// as of this tick.
    way_users: Vec<WayUser>,
    /// The LAN players' vehicles (their session ids and boxes as for the player), set
    /// before each `tick`: the cars stop behind them and go round them as round the
    /// player's bus.
    pub others: Vec<(u32, PlayerBox)>,
    /// The drivers at the wheel of the timetable buses near the camera, by car id (see
    /// `driver.rs`; made within `DRIVER_NEAR` m of the camera, let go beyond twice that).
    drivers: HashMap<u64, crate::driver::DriverFigure>,
    /// Figures let go by their bus, hidden, for the next one (their GPU meshes stay).
    driver_pool: Vec<crate::driver::DriverFigure>,
    /// Where the last `tick` spent its time (s, OMSI_PROFILE): who is on which lane and the
    /// light programs, every car's plan, the bodies and scripts on the workers.
    pub tick_split: [f64; 3],
    /// Seconds each of them has stood still.
    others_still: HashMap<u32, f32>,
    /// Per car: `AiCar::geo_block` of the frame before (who waits for whom by geometry).
    geo_prev: Vec<Option<u64>>,
    /// Car index by id (as of the start of the tick).
    index_of: HashMap<u64, usize>,
    /// `pull_out_room` by vehicle file.
    pull_out_rooms: HashMap<std::path::PathBuf, f32>,
    /// Timetable buses taken off the road because the tile under them was unloaded (their
    /// ids), for the timetable to put them back when the tiles come again.
    pub removed_scheduled: Vec<u64>,
    /// One-way lanes that have had their reverse twin added (`add_reverse_twins`).
    twinned: hashbrown::HashSet<usize>,
    /// Bodies besides the AI vehicles' that no timetable vehicle may be put into: the
    /// player's vehicle and the LAN players' (set before each `Schedule::tick`).
    pub keep_clear: Vec<omsi_sim::collision::Obb>,
    /// LAN play: this game draws the host's traffic instead of its own (`lan_world`).
    mirror: bool,
    /// Count only the cars within this distance of this point when filling up (the
    /// population around a LAN player, `populate_lan_centers`).
    count_near: Option<(DVec3, f64)>,
    /// LAN play: where the other players are (host): the traffic is kept around them too.
    pub lan_centers: Vec<DVec3>,
}
