//! The AI cars and what the simulation keeps about them, and the helpers that put a
//! vehicle on its way.

use super::*;

/// Pulling out onto the other half of the road round something standing in the lane.
#[derive(Debug, Clone, Copy)]
pub struct Passing {
    /// The lane of the oncoming traffic the car moves over onto.
    pub lane: usize,
    /// How far to the left that lane lies (m).
    pub side: f32,
    /// Odometer reading at which the car is past the obstacle and moves back.
    pub until: f32,
    /// Odometer reading at which the car's front would reach the obstacle (had it stayed in
    /// its lane): until then it can still give up and stop behind it.
    pub block: f32,
    /// Length of the S-curve back into the lane (m).
    pub back: f32,
    /// Given up because somebody came the other way: back in and stopping at the odometer
    /// reading `hold`.
    pub aborted: bool,
    pub hold: f32,
    /// Started from a standstill close behind the obstacle: the car edges out
    /// (`PULL_OUT_ACCEL`) until its front is past the obstacle's corner.
    pub creep: bool,
}

impl Passing {
    /// Odometer reading at which the car has moved back far enough to be out of the way of
    /// the oncoming traffic (`half_width` its own half width).
    pub(super) fn clear_at(&self, half_width: f32) -> f32 {
        self.until
            + self.back
                * omsi_sim::traffic::ramp_progress_for(self.side, half_width + ONCOMING_ROOM)
    }
}

/// Room an oncoming vehicle needs beside a car (m from the car's side to the middle of the
/// oncoming lane): its half width and a margin.
pub(super) const ONCOMING_ROOM: f32 = 1.15;

/// What makes a new car a timetable bus (`Traffic::create_car`).
pub struct BusSetup {
    /// The trip's lanes, as far as the loaded tiles have them.
    pub route: Vec<usize>,
    pub stops: Vec<crate::bus_service::Stop>,
    /// Fleet number and registration (`number`, `ident` string variables).
    pub number: Option<(String, String)>,
    pub hof: Option<Arc<omsi_vehicle::Hof>>,
    pub timetable: crate::bus_service::AiTimetable,
}

pub struct AiCar {
    /// Stable id for references from other systems (passengers).
    pub id: u64,
    /// The random seed it was made with and its paint scheme: a car that goes out of range
    /// and comes back is the same car (`DormantCar`).
    pub seed: u64,
    pub scheme: Option<usize>,
    pub state: AiState,
    pub vehicle: VehicleInstance,
    /// What it is drawn as (see `DrawnAs`); the pictures themselves are `TrafficView`'s.
    pub render: DrawnAs,
    /// The body following the way `state` lays out.
    pub body: AiBody,
    /// Seconds this car has been standing still without a stop of its own: a red light or
    /// a queue is seconds, a jam that never clears grows without bound.
    pub stopped: f32,
    /// The car it follows now (its id), when one is close ahead.
    pub lead_car: Option<u64>,
    /// A car it does not take for its lead until the time given: two that had each other
    /// for their lead (see `Traffic::break_lead_pairs`).
    pub ignore_lead: Option<(u64, f64)>,
    /// Seconds it has crept along below 1 m/s (a claim of one that crawls in a jam of its
    /// own is no car about to come either).
    pub crawl: f32,
    /// The odometer when it last got two metres further, and the seconds since: a car
    /// that creeps against something it never gets past stands as much as one that stops
    /// (`stopped` starts afresh with every centimetre it creeps).
    pub progress: (f32, f32),
    /// A timetable bus: its trip's stops, the doors, the layover, the people aboard (see
    /// `bus_service`). Everything else about it is this car's.
    pub bus: Option<Box<BusService>>,
    /// `[sound_ai]` set, created when the car comes near the listener.
    pub sounds: Option<omsi_audio::SoundSet>,
    /// Half the vehicle's width (m).
    pub half_width: f32,
    /// Waiting at a junction for someone with the right of way this frame.
    pub yielding: bool,
    /// Stopped by a red light this frame.
    pub light_hold: bool,
    /// Junction lanes this car has claimed to drive through (`TPathInfo::reservePaths`).
    pub reserved: Vec<usize>,
    /// The light (controller, lamp) the driver decided to pass on yellow.
    pub amber: Option<(usize, usize)>,
    pub passing: Option<Passing>,
    /// Finished (a dead end, the end of a timetable trip, given up): taken off the road as
    /// soon as nobody can see it.
    pub gone: bool,
    /// Seconds since it was put on the road are fewer than this: its speed was a guess.
    pub fresh: f32,
    /// The car it lets go first at the next merge (by id).
    pub merge_after: Option<u64>,
    /// What holds it (`OMSI_DEBUG_TRAFFIC`, for cars standing for long).
    pub holding: Option<String>,
    /// What held the car back this frame (for OMSI_TRACE_AI): the constraint nearest ahead
    /// ("lead", "light", "yield", "merge", "keep_back", "people", "pull_out", "service",
    /// "end", "" for none) and its distance ahead of the front (m).
    pub why: (&'static str, f32),
    /// Something made it wait this frame: a stop point, or a car or an obstacle close ahead.
    pub held: bool,
    /// The vehicle (by id) whose body stands in this car's way off its lanes this frame
    /// (`Traffic::body_in_way`).
    pub geo_block: Option<u64>,
    /// What it keeps behind (by id) and the gap to it, as of its last step.
    pub lead_info: Option<(u64, f32)>,
    /// What it waited for at its last junction (`OMSI_DEBUG_STUCK` only).
    pub junction_why: String,
    /// Giving way: where it waits (distance from its origin to the line).
    pub wait_at: Option<f32>,
    /// The vehicle (by id) standing half out of the lane that this car is squeezing past.
    pub squeeze: Option<u64>,
    /// How far behind something standing (a bus at its stop, the player's bus) this car
    /// stops, so that it can steer out round it later (m, front bumper to the other's body;
    /// from its own steering, `pull_out_room`).
    pub pass_room: f32,
    /// No new look at passing before this time (a pull-out that did not clear the corner is
    /// not tried again every frame).
    pub pass_retry: f32,
    /// The traffic light it waited for in the last frame: distance from its origin.
    pub light_at: Option<f32>,
    /// A car that was parked at the kerb: seconds it still stands there, indicating, before
    /// it pulls out (see `Traffic::pull_out_parked`).
    pub pull_out: f32,
    /// Parking: the free space it drives into (see `Traffic::park_in`).
    pub park: Option<ParkPlan>,
    /// A rail vehicle: the track it has come along, (odometer, point), oldest first -
    /// where its rear bogie and its coupled cars and sections run (see `rail_behind`).
    pub rail_trail: std::collections::VecDeque<(f64, DVec3)>,
    /// Seconds its body and script took last frame (heavy ones get an AI job of their own).
    pub ai_secs: f32,
    /// A train turned round as a whole (its last car leads now): what a trip's
    /// `[trainreverse]` is compared with (Omsi.exe's vehicle +0x4e1).
    pub consist_reversed: bool,
}

/// What an AI car is drawn as, for whoever describes it to others (a LAN host's cars to
/// its clients): the vehicle file and the paint scheme of the shared GPU set it is drawn
/// with (`VehicleRender::set`).
#[derive(Debug, Clone, Default)]
pub struct DrawnAs {
    pub set: Option<(std::path::PathBuf, Option<usize>)>,
}

/// A free parking space beside a lane that a car means to park in: the space of parked car
/// `key` that drove off (its object comes back when the car is in).
#[derive(Debug, Clone, Copy)]
pub struct ParkPlan {
    pub key: i64,
    pub lane: usize,
    /// The space's middle along the lane and its offset to the right of it (m).
    pub s: f32,
    pub lat: f32,
    /// Moving over into the space.
    pub ramped: bool,
    /// In the space and standing: the parked object takes its place at the next sync.
    pub done: bool,
}

impl AiCar {
    /// A timetable bus (in service or on its way off after its trip).
    pub fn is_bus(&self) -> bool {
        self.bus.is_some()
    }

    /// Bound to rails (a train, a tram).
    pub fn is_rail(&self) -> bool {
        self.body.kind == MotionKind::Rail
    }

    /// Boarding at a stop: the script is told to open the doors (`AI_Scheduled_AtStation`).
    pub fn at_station(&self) -> bool {
        self.bus.as_ref().map(|b| b.at_station()).unwrap_or(false)
    }

    /// The side's doors to open at the stop it is boarding at (`AI_Scheduled_AtStation_Side`).
    pub fn at_station_side(&self) -> f32 {
        self.bus.as_ref().map(|b| b.at_station_side()).unwrap_or(0.0)
    }

    /// Standing at one of its stops (doors open, waiting for the departure, pulling out).
    pub fn at_stop(&self) -> bool {
        self.bus.as_ref().map(|b| b.at_stop()).unwrap_or(false)
    }

    pub fn trip_done(&self) -> bool {
        self.bus.as_ref().map(|b| b.trip_done()).unwrap_or(false)
    }

    pub fn route_open(&self) -> bool {
        self.bus.as_ref().map(|b| b.route_open).unwrap_or(false)
    }

    /// The next stop: (route index, distance along that lane).
    pub fn next_stop(&self) -> Option<(usize, f32)> {
        self.bus.as_ref()?.stops.front().map(|s| (s.ri, s.s))
    }

    /// Seconds it will still stand at its stop.
    pub fn standing_for(&self, day_time: f64) -> f32 {
        self.bus.as_ref().map(|b| b.standing_for(day_time)).unwrap_or(0.0)
    }
}

/// Where a vehicle's body stands, for the checks that go by geometry rather than by lanes:
/// the car it belongs to (a trailer or rear section counts as its own footprint), centre,
/// forward and right unit vectors, half length, half width and speed along its heading.
#[derive(Debug, Clone, Copy)]
pub(super) struct Footprint {
    pub(super) car: usize,
    pub(super) center: DVec2,
    pub(super) fwd: DVec2,
    pub(super) right: DVec2,
    pub(super) half_len: f64,
    pub(super) half_w: f64,
    pub(super) speed: f32,
    /// Height of the vehicle's origin (an aircraft overhead is not in a car's way).
    pub(super) z: f64,
}

/// A car's own footprint, grown forward by `ahead` metres.
pub(super) fn car_foot(c: &AiCar, ahead: f32) -> Footprint {
    let st = &c.state;
    let h = c.vehicle.heading.to_radians();
    let (fwd, right) = (DVec2::new(h.sin(), h.cos()), DVec2::new(h.cos(), -h.sin()));
    let center = c.vehicle.position.truncate() + fwd * ((st.front + ahead - st.rear) * 0.5) as f64;
    Footprint { car: usize::MAX, center, fwd, right, half_len: ((st.front + ahead + st.rear) * 0.5) as f64, half_w: c.half_width as f64, speed: st.speed, z: c.vehicle.position.z }
}

impl Footprint {
    pub(super) fn from_obb(car: usize, b: &omsi_sim::collision::Obb, speed: f32) -> Footprint {
        let (sh, ch) = (b.heading.sin(), b.heading.cos());
        Footprint {
            car,
            center: b.center,
            fwd: DVec2::new(sh, ch),
            right: DVec2::new(ch, -sh),
            half_len: b.half.y,
            half_w: b.half.x,
            speed,
            z: b.z0,
        }
    }

    /// The footprint as a collision box (no height range).
    pub(super) fn obb(&self) -> Obb {
        Obb {
            center: self.center,
            half: DVec2::new(self.half_w, self.half_len),
            heading: self.fwd.x.atan2(self.fwd.y),
            z0: f64::MIN,
            z1: f64::MAX,
            velocity: DVec2::ZERO,
            mass: 0.0,
            pole: None,
            id: -1,
        }
    }

    /// Does this footprint overlap `o`, both grown by `margin` (separating axes)?
    pub(super) fn overlaps(&self, o: &Footprint, margin: f64) -> bool {
        let d = o.center - self.center;
        for axis in [self.fwd, self.right, o.fwd, o.right] {
            let extent = |f: &Footprint| {
                (f.fwd.dot(axis)).abs() * (f.half_len + margin)
                    + (f.right.dot(axis)).abs() * (f.half_w + margin)
            };
            if d.dot(axis).abs() > extent(self) + extent(o) {
                return false;
            }
        }
        true
    }
}

/// The player's vehicle as the traffic sees it: centre, heading (deg), half length, half
/// width, speed along the heading (m/s, negative when reversing).
pub type PlayerBox = (DVec3, f64, f32, f32, f32);

/// A random car out of the player's range: still on the map and still driving, but without
/// a body, a script or a picture - a few numbers. It comes back as the same car (type,
/// paint, id) where it has got to when the player comes near, and it only ever leaves the
/// map at the end of the road network. Before, every car out of range was simply taken
/// away and new ones made up around the player: the traffic followed the player about, and
/// a car driven past was never seen again.
pub struct DormantCar {
    pub id: u64,
    pub ty: Arc<VehicleType>,
    pub kind: LaneKind,
    pub lane: usize,
    pub s: f32,
    pub speed: f32,
    pub seed: u64,
    pub scheme: Option<usize>,
    /// Its own dice for the turns it takes.
    pub walk: u64,
}

/// `[boundingbox]` of a vehicle that gives none.
pub(super) const DEFAULT_BOX: [f32; 6] = [2.5, 12.0, 3.0, 0.0, 0.0, 1.5];

/// The bodies of a vehicle and of the parts coupled to it, where they stand.
pub fn vehicle_bodies(v: &VehicleInstance) -> Vec<omsi_sim::collision::Obb> {
    let mut out = vec![omsi_sim::collision::Obb::from_box(
        v.ty.def.bounding_box.unwrap_or(DEFAULT_BOX),
        v.position,
        v.body_heading(),
    )];
    for t in &v.trailers {
        out.push(omsi_sim::collision::Obb::from_box(
            t.ty.def.bounding_box.unwrap_or(DEFAULT_BOX),
            t.position,
            t.body_heading(),
        ));
    }
    out
}

/// A timetable bus's IBIS moves on to its next stop as the driver would press it on: the
/// stock scripts' interior displays, announcements and side displays read `IBIS_busstop`
/// (an index into the depot file's stop list of the route), which nothing moved on an AI
/// bus - its saloon display stood on the first stop for the whole trip. `remaining` is the
/// number of stops still to come.
pub(crate) fn ibis_to_next_stop(v: &mut VehicleInstance, remaining: usize) {
    let Some(ri) = v.var("IBIS_RouteIndex").filter(|r| *r >= 0.0) else { return };
    let Some(n) = v.host.hof.as_ref().and_then(|h| h.info_busstop_lists.get(ri as usize)).map(|l| l.len()) else { return };
    if n == 0 || v.var("IBIS_busstop").is_none() {
        return;
    }
    let idx = n.saturating_sub(remaining.max(1)).min(n - 1);
    v.set_var("IBIS_busstop", idx as f32);
}

/// How a vehicle on lanes of `kind` moves.
pub(super) fn motion_kind(kind: LaneKind) -> MotionKind {
    match kind {
        LaneKind::Air => MotionKind::Air,
        LaneKind::Rail => MotionKind::Rail,
        _ => MotionKind::Road,
    }
}

/// How much track an AI rail vehicle keeps behind it (m): a long train's length.
pub(super) const RAIL_TRAIL: f64 = 400.0;

/// Note where an AI rail vehicle is: `odometer` (m) and the point of its way there. A jump
/// (put somewhere else, turned round at a terminus) starts the trail afresh.
pub(super) fn record_rail_trail(trail: &mut std::collections::VecDeque<(f64, DVec3)>, odometer: f64, here: DVec3) {
    if let Some(&(u, p)) = trail.back() {
        if (here - p).truncate().length() > (odometer - u).abs() + 2.0 {
            trail.clear();
        } else if (odometer - u).abs() <= 0.5 {
            return;
        }
    }
    // (backing up takes the trail back with it)
    while trail.back().is_some_and(|b| b.0 > odometer) {
        trail.pop_back();
    }
    trail.push_back((odometer, here));
    while trail.front().is_some_and(|f| odometer - f.0 > RAIL_TRAIL) {
        trail.pop_front();
    }
}

/// The point of an AI rail vehicle's track `d` metres behind its origin: on the trail it
/// came along. (Its way knows only the lane it came off; farther back it runs straight on,
/// and a train's last cars stood beside the track after a pair of points.) Where the trail
/// does not reach - the last half metre, a vehicle just put there - the way.
pub(super) fn rail_behind(trail: &std::collections::VecDeque<(f64, DVec3)>, state: &AiState, net: &Network, d: f64) -> DVec3 {
    let u = state.odometer as f64 - d;
    let newest = trail.back().map_or(f64::MIN, |b| b.0);
    if u >= newest {
        return state.way_point(net, -d as f32);
    }
    crate::rail_drive::point_at(trail, u).unwrap_or_else(|| state.way_point(net, -d as f32))
}

/// A body for a vehicle that has just been put on the way `state` describes, with the
/// vehicle posed on it.
pub(super) fn place_body(
    net: &Network,
    state: &AiState,
    vehicle: &mut VehicleInstance,
    kind: MotionKind,
) -> AiBody {
    let mut body = AiBody::new(&vehicle.ty.def, kind);
    let ground = vehicle.ground.clone();
    let contact = vehicle.contact.clone();
    body.place(
        &|d| state.way_point(net, d),
        ground
            .as_ref()
            .map(|g| g.as_ref() as &dyn Fn(f64, f64) -> Option<f64>),
        contact.as_deref(),
        state.speed,
    );
    body.apply(vehicle);
    body
}

/// The vehicle's extent from its origin: (to the front bumper, to the rear bumper, half
/// the width) from its `[boundingbox]`.
pub(super) fn extents(ty: &VehicleType, length: f32) -> (f32, f32, f32) {
    let (front, rear, width) = match ty.def.bounding_box {
        Some(bb) if bb[1] > 1.0 => (
            bb[1] * 0.5 + bb[4],
            bb[1] * 0.5 - bb[4],
            (bb[0] * 0.5).max(0.5),
        ),
        // without a `[boundingbox]` the model's own box, as Omsi.exe takes it (0x7b5da4):
        // the Berlin S-Bahn's cars, 18 m long, counted as 12 m ones
        _ => match ty.model_box() {
            Some((lo, hi)) if hi.y - lo.y > 1.0 => (hi.y.max(0.5), (-lo.y).max(0.5), (hi.x.max(-lo.x)).max(0.5)),
            _ => (length * 0.5, length * 0.5, 0.9),
        },
    };
    if omsi_sim::vehicle::body_reversed(&ty.def, false) {
        (rear, front, width)
    } else {
        (front, rear, width)
    }
}

/// The driver of a random car: how fast, how close, how patient (see `AiState`).
pub(super) fn personality(state: &mut AiState, seed: u64, heavy: bool) {
    let r = |k: u32| ((seed >> k) & 0xff) as f32 / 255.0;
    state.desire = if heavy {
        0.88 + 0.1 * r(3)
    } else {
        0.9 + 0.22 * r(3)
    };
    state.headway = 1.0 + 0.8 * r(11);
    state.min_gap = 1.6 + 1.4 * r(19);
    state.accel = if heavy {
        0.8 + 0.4 * r(27)
    } else {
        1.3 + 1.0 * r(27)
    };
    state.decel = if heavy { 1.6 } else { 2.0 + 0.8 * r(35) };
    state.accept_gap = 3.0 + 2.5 * r(43);
    state.reaction = 0.4 + 0.8 * r(51);
}
