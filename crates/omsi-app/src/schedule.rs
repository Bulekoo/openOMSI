//! Scheduled AI buses: the map's timetable lines (`TTData`) put buses on their tracks at the
//! tour departure times; they follow the track lanes and stop at the trip's stations.
//!
//! The timetable itself (`load`, `times`, `route`, `tours`, `boards`, `ibis`, `duty`) does
//! not touch the GPU; `fleet`, `dispatch` and `spawn` upload the vehicles and put them on the
//! road.

mod boards;
mod dispatch;
mod duty;
mod fleet;
mod ibis;
mod load;
mod route;
mod spawn;
mod times;
mod tours;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod authored_station_tests;

use crate::scene::World;
use crate::traffic::Traffic;
use hashbrown::{HashMap, HashSet};
use omsi_sim::traffic::{LaneKey, Network};
use omsi_sim::VehicleType;
use omsi_timetable::TimetableData;
use std::path::Path;
use std::sync::Arc;

use dispatch::*;
use duty::*;
use fleet::*;
use ibis::*;
use route::*;
use times::*;

pub(crate) use boards::hhmm;
pub(crate) use duty::ibis_stop_index;
pub(crate) use ibis::{
    has_roller_blind, player_ibis, set_ai_destination, set_ai_destination_at,
    set_player_destination_at, set_player_destination_directly, shown_destination,
    turn_roller_blind, BlindPick,
};

struct Departure {
    /// Seconds since midnight.
    time: f64,
    trip: usize,
    /// The trip's profile the tour runs it with (`[addtrip]`).
    profile: usize,
    line: String,
    ai_group: String,
    tour: String,
    /// The tour's validity mask (bits 0-6 Monday..Sunday, 7 public holiday, 8 school
    /// holidays, 9 school days): every tour's departures are kept, and which run is decided
    /// by the day (`Schedule::runs`), so a session carries on past midnight.
    mask: i32,
    spawned: bool,
}

/// One step of a trip's route: a lane in map terms (None when the tile index is not in the
/// map's list) and the leg between two stations it belongs to (0 for a track).
#[derive(Debug, Clone, Copy)]
struct Step {
    key: Option<LaneKey>,
    leg: usize,
    /// The path's length as the timetable file has it (m).
    length: f64,
}

/// A route step as the loaded network has it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Slot {
    /// The lane it runs on.
    Lane(usize),
    /// Its tile is part of the map, but has not brought its lanes yet.
    Waiting,
    /// Not in the map (a tile or path that does not exist): passed over, as a whole-map load
    /// passes it over.
    Absent,
}

/// What became of a departure that was due.
enum Placed {
    Spawned,
    /// The part of the route the bus is on now is not loaded: try again later.
    Wait,
    /// Nothing to do any more (the trip is over, or has no route or no vehicle).
    Drop,
    /// A car stands where the bus would appear: try again at the next call.
    Busy,
}

/// A scheduled bus whose route stops short of a tile that has not brought its lanes yet: the
/// route is carried on as the tiles come.
struct RunningTrip {
    car: u64,
    steps: Vec<Step>,
    /// The first step its route does not have yet.
    next: usize,
    /// The trip's stations with their departure times, and which of them the bus stops at
    /// already or has passed.
    stations: Vec<(i64, f64)>,
    served: Vec<bool>,
    /// Authored track entry of each type-1 station, retained while tiles stream in.
    station_steps: Vec<Option<usize>>,
}

/// When a trip's bus is at each of its stations, as OMSI's timetable has it: the profile
/// gives the trip's duration and, for some stations, the minute the bus arrives or leaves
/// (`[profile_man_arr_time]`, `[profile_man_dep_time]`, minutes after the trip's start);
/// the stations in between are timed by the lengths of the station links (Spandau's line 5
/// gives nearly every station its minute; these used to be ignored for the whole duration
/// split by the link lengths, and the tours ran their first profile whatever `[addtrip]`
/// said). A station marked `[profile_otherstopping] 2` is passed without a stop (every
/// station of a depot run).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TripTimes {
    /// Seconds after the trip's departure: (arrival, departure) per station.
    pub stations: Vec<(f64, f64)>,
    /// Whether the bus stops at the station.
    pub stops: Vec<bool>,
    /// `[profile_otherstopping]` per station (0 when not given): 1 and 4 stop whoever
    /// wants to get on or off, 2 is passed, 3 is served when the bus would be more than 20 s
    /// early (Omsi.exe 0x7da6f0 .. 0x7da8bf; see `bus_service::BusService::must_serve`).
    pub kinds: Vec<u8>,
    /// Seconds from the departure to the arrival at the last station.
    pub duration: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum StopRoute {
    Nearest,
    Track(usize),
    Outside,
}

/// How long before a trip is due at a stop the people for it turn up there (s).
pub const PAX_SPAWN_AHEAD: f64 = 15.0 * 60.0;

/// How far from a route a bus stop may stand when the route is only a part of the trip (a
/// stop of the missing part would otherwise be put on the nearest point of this one).
const STOP_REACH: f64 = 25.0;

/// The vehicle a departure is driven with. A tour keeps its bus all day, as in OMSI: the
/// choice comes from the tour, not from the order the departures happen to spawn in, so the
/// timetable knows which vehicles its next minutes need before they are due.
struct Choice {
    ty: Arc<VehicleType>,
    number: Option<(String, String)>,
    hof: Option<Arc<omsi_vehicle::Hof>>,
    scheme: Option<usize>,
    /// A `.zug` train: its cars, the first one being `ty`.
    train: Option<Vec<(Arc<VehicleType>, bool)>>,
}

pub struct Schedule {
    pub data: TimetableData,
    departures: Vec<Departure>,
    /// Depot vehicles per AI group: (type, its fleet from the ailists, depot file).
    depots: HashMap<
        String,
        Vec<(
            Arc<VehicleType>,
            Vec<omsi_map::DepotEntry>,
            Option<Arc<omsi_vehicle::Hof>>,
        )>,
    >,
    tile_coords: Vec<(i32, i32)>,
    next_number: usize,
    /// Trains per AI group: list of (car type, reversed), first car leads.
    trains: HashMap<String, Vec<Vec<(Arc<VehicleType>, bool)>>>,
    /// Plain `[aigroup_2]` vehicle pools, loaded the first time a trip asks for one
    /// (the Tegel approaches are flown by the group's own aircraft, not by depot buses).
    pools: HashMap<String, Vec<Arc<VehicleType>>>,
    /// Per vehicle folder: the depot file a bus of a plain `[aigroup_2]` runs with
    /// (see [`pool_depot`]), looked up the first time such a bus is chosen.
    pool_hofs: HashMap<std::path::PathBuf, Option<Arc<omsi_vehicle::Hof>>>,
    /// Departures that are due but not on the road yet. Putting twenty minutes of a Berlin
    /// timetable on the map at once costs several seconds in one frame, so they are spawned
    /// a few at a time.
    pending: std::collections::VecDeque<usize>,
    /// Per departure: the previous departure of the same tour (its bus is the same one).
    tour_prev: Vec<Option<usize>>,
    /// Due departures whose bus would be on a part of its route that is not loaded: tried
    /// again when tiles bring lanes and as time moves the bus on.
    waiting: Vec<usize>,
    /// Buses on the road whose route is still to be carried on.
    running: Vec<RunningTrip>,
    /// `Traffic::lanes_generation` when the waiting departures and the running routes were
    /// last looked at, and the time of day of the last retry.
    seen_generation: u64,
    last_retry: f64,
    /// The departure each timetable bus on the road runs (by car id): a bus the traffic took
    /// off with its unloaded tile goes back to `waiting` and returns with the tiles, at the
    /// place its timetable puts it then.
    car_departure: HashMap<u64, usize>,
    /// Waiting departures whose vehicle is timed to reach loaded lanes at this time of day:
    /// they are tried again then, so that a plane coming in over tiles nobody loads appears
    /// where its path enters the loaded ones, not up to half a minute later in mid-air.
    retry_at: HashMap<usize, f64>,
    /// Vehicle sets being read ahead on the workers, with the type to upload them with, and
    /// those read and waiting for their upload.
    fleet_reading: HashMap<crate::scene::VehicleKey, Arc<VehicleType>>,
    fleet_ready: Arc<parking_lot::Mutex<Vec<crate::scene::VehicleKey>>>,
    /// Time of day of the last look at the next departures' vehicles.
    fleet_check: f64,
    /// Per trip and profile: when its bus is at its stations.
    times: Vec<Vec<TripTimes>>,
    /// Per bus stop (map object id): the trips that call there, as (trip, station index).
    visits: HashMap<i64, Vec<(usize, usize)>>,
    /// Per trip: today's departures that run it.
    trip_departures: Vec<Vec<usize>>,
    /// When the departure boards were last made (time of day).
    boards_made: f64,
    /// The tour the player drives (line, tour): the timetable does not run it as well.
    player_tour: Option<(String, String)>,
    /// With a single trip picked: the departure (s of the day) of that trip; the rest of
    /// the tour stays the AI's.
    player_departure: Option<f64>,
    /// A player tour was just taken over: its buses already on the road go at the next tick.
    purge_player_tour: bool,
    /// LAN play (host): the tours the other players drive (line, tour; lower case), left to
    /// them like our own.
    lan_tours: HashSet<(String, String)>,
    /// Time of today's timetable at the last tick (s).
    last_tod: f64,
    /// The lines the date's chrono folders take off the timetable, with the folder that does
    /// it: why a duty on such a line cannot be driven.
    deactivated: Vec<(String, std::path::PathBuf)>,
    /// Stations some trip stops at on its way or ends at (not only starts from).
    served: std::collections::HashSet<i64>,
    /// Per first station: whether another trip's bus stops there or within a bus length
    /// or two of it (maps often put one stop object per line at the same kerb), once its
    /// position is known.
    shared_stand: HashMap<i64, bool>,
    /// Departures queued while the map loads: their buses may appear in view.
    startup: std::collections::HashSet<usize>,
    /// Layover departures whose stand was taken: they come at their departure time.
    later_layover: std::collections::HashSet<usize>,
    /// Per departure: the next departure of the same tour (its bus takes it on).
    tour_next: Vec<Option<usize>>,
    /// Departures due while their tour's bus is still on its previous trip: that bus takes
    /// them on when it gets there, as in OMSI a tour keeps its bus from trip to trip.
    awaiting: std::collections::HashSet<usize>,
    /// The clock was set (`restart`): the next tick puts the buses out as a start does.
    restarted: bool,
    /// The map's holidays, for the day's tours.
    calendar: omsi_map::Calendar,
    /// The date (yyyymmdd) the departures are for, and the mask bits it selects (day,
    /// school); `set_day` moves them on at midnight.
    day: i32,
    day_bits: (i32, i32),
    /// The weekday bit of the next day (night tours run on into it).
    next_day_bit: i32,
    /// Where today's midnight lies on the traffic's clock (`Traffic::day_time` counts on past
    /// 24:00): a departure leaves at `day_base + time` (`dep_time`), and the date moves on
    /// when the clock passes the next midnight.
    day_base: f64,
    /// The current date (its time of day is not used).
    date_clock: omsi_sim::SimClock,
    /// The map's `car_use/*.ocu`: which vehicles serve which line's tours.
    car_use: Vec<omsi_timetable::CarUse>,
    /// Per tour (`tour_key_of`): the depot vehicle (index in its group) and fleet number
    /// it runs with today - from `car_use`, else drawn the first time the tour is due. A
    /// number is given to one tour only (`used_numbers`), as in OMSI:
    /// hashing each tour to a number put the same fleet number on two buses at once.
    tour_vehicle: HashMap<u64, (usize, usize)>,
    used_numbers: HashSet<(String, String)>,
}

/// A tour's bus waits at the end of a trip for the next one of its tour when that leaves
/// within this many seconds; for a longer break it goes (and a bus comes back for it).
const TOUR_LAYOVER_MAX: f64 = 30.0 * 60.0;

/// How early a bus waits at its first stop for its departure (s): a quarter of an hour at a
/// stand of its own, a minute where other buses stop as well (a layover bus there made
/// every bus of the other lines queue behind it until it left).
const LAYOVER: f64 = 900.0;

const LAYOVER_SHARED: f64 = 60.0;

/// One stop of a planned trip with its scheduled times (seconds since midnight).
#[derive(Debug, Clone)]
pub struct PlannedStop {
    pub object_id: i64,
    pub name: String,
    pub arr: f64,
    pub dep: f64,
    pub position: Option<glam::DVec3>,
    /// Which way the trip runs through the stop ([`StopDir`]): a circular route, or one
    /// that turns back, calls at the same place twice and the two stops of it stand a few
    /// metres apart. Only the direction says which of them a bus has reached (#254).
    pub dir: StopDir,
    /// The bus stops here (a depot run passes its stations).
    pub stops: bool,
}

/// Which way a trip runs through one of its stops: the direction it arrives on and the one
/// it leaves on, as unit vectors of the ground plane (x east, y north). None where a
/// neighbour's place is unknown or too near to tell a direction - any heading will do then.
#[derive(Debug, Clone, Copy, Default)]
pub struct StopDir {
    pub inbound: Option<glam::DVec2>,
    pub outbound: Option<glam::DVec2>,
}

#[derive(Debug, Clone)]
pub struct PlannedTrip {
    pub name: String,
    pub line: String,
    pub terminus: String,
    pub departure: f64,
    /// Arrival at the last station.
    pub end: f64,
    pub stops: Vec<PlannedStop>,
}

/// The bus is at a stop within this distance (m), and has left it beyond the second.
const AT_STOP: f64 = 25.0;

/// How long before its departure the next trip of a duty may begin when the bus leaves the
/// terminus it has served (s).
const EARLY_START: f64 = 300.0;

const LEFT_STOP: f64 = 35.0;

/// How far ahead (s) the departure displays look.
const BOARD_AHEAD: f64 = 2.0 * 3600.0;

/// Most departures a page gets for a stop (`omsi.getDepartures`).
const MAX_PAGE_DEPARTURES: usize = 20;

/// The player's tour: its trips with planned stop times, and the progress along them.
pub struct PlayerDuty {
    pub line: String,
    pub tour: String,
    pub trips: Vec<PlannedTrip>,
    pub trip_index: usize,
    /// Where `trips` begins in the tour: a picked trip is a duty of its own, and a saved
    /// situation counts the trip under way from the tour's first.
    pub first_trip: usize,
    /// Next stop to serve on the current trip.
    pub next_stop: usize,
    /// True while the bus stands at the next stop.
    at_stop: bool,
    /// How late the bus arrived at the stop it stands at (s after its arrival time).
    arrived_late: Option<f64>,
    /// The bus has reached the last stop of the current trip.
    done: bool,
    /// The last stop where this trip actually stopped with a passenger door open.
    served_terminus: Option<glam::DVec3>,
    /// How late (s, negative = early) the bus left the last stop it served on this trip;
    /// None while it has not left one.
    left_late: Option<f64>,
    /// A page moved the duty back to an earlier stop: `catch_up` must not jump forward
    /// again to a later stop the bus still stands at, until the bus reaches a stop again.
    held_back: bool,
    /// The first update looks where the bus stands.
    placed: bool,
    /// The current trip changed since the last `take_trip_change`.
    trip_changed: bool,
    /// Stops the bus passed without stopping since the last `take_skipped` (see `catch_up`).
    skipped: Option<(usize, usize, usize)>,
    /// The player picked the current trip: the duty does not move on past it before it is
    /// driven (or given up), however late the bus is for it.
    picked: bool,
    /// Time of day of the first update (placing waits a little for the places of stops
    /// beyond the loaded tiles, see `learn_places`).
    first_update: Option<f64>,
    /// The way the bus faces (degrees clockwise from north), from the last update: it says
    /// which of two stops a few metres apart the bus is at (see `StopDir`).
    heading: f64,
}

const DAY: f64 = 86_400.0;
