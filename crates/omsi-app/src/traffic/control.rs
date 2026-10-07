//! What the timetable and the rest of the game ask of the traffic and tell it.

use super::*;
use crate::scene::World;
use omsi_render::{Renderer, Scene};

impl Traffic {
    /// Would a vehicle of type `ty` with its origin at `pos`, heading `heading` (and its
    /// coupled parts, straight behind it) touch one that is already there - an AI vehicle,
    /// or one of `keep_clear`? Bodies are compared with half a metre to spare, not centres:
    /// an articulated bus reaches 6 m ahead of its origin and 12 m behind it, and one put
    /// down 9.3 m from the player's bus stood 1.8 m inside it.
    pub fn blocked(&mut self, ty: &Arc<VehicleType>, pos: DVec3, heading: f64) -> bool {
        let grown = |mut b: omsi_sim::collision::Obb| {
            b.half += glam::DVec2::splat(0.5);
            b
        };
        let mut bodies = vec![grown(omsi_sim::collision::Obb::from_box(
            ty.def.bounding_box.unwrap_or(DEFAULT_BOX),
            pos,
            omsi_sim::vehicle::body_heading(&ty.def, heading, false),
        ))];
        let (mut origin, mut lead, mut lead_rev) = (pos, ty.clone(), false);
        for (t, rev) in self.trailer_chain(ty) {
            let (back, front) = omsi_sim::vehicle::coupling_points(&lead, lead_rev, &t, rev);
            // each car stands along the consist's heading, turned round by its own
            // (absolute) orientation - never by the car in front of it
            let (center, car_heading) = omsi_sim::vehicle::coupling_placement(
                origin,
                heading,
                omsi_sim::vehicle::body_reversed(&lead.def, lead_rev),
                back.y,
                omsi_sim::vehicle::body_reversed(&t.def, rev),
                front.y,
            );
            bodies.push(grown(omsi_sim::collision::Obb::from_box(
                t.def.bounding_box.unwrap_or(DEFAULT_BOX),
                center,
                car_heading,
            )));
            origin = center;
            lead = t;
            lead_rev = rev;
        }
        let reach = bodies
            .iter()
            .map(|b| (b.center - pos.truncate()).length() + b.half.length())
            .fold(0.0, f64::max)
            + 40.0;
        let touches = |o: &omsi_sim::collision::Obb| bodies.iter().any(|b| b.overlaps(o));
        self.keep_clear.iter().any(|o| touches(o))
            || self
                .cars
                .iter()
                .filter(|c| (c.vehicle.position - pos).length() < reach)
                .any(|c| vehicle_bodies(&c.vehicle).iter().any(|o| touches(o)))
    }

    /// The lanes the other way along one-way paths `lanes` (see `Schedule`'s route
    /// building: OMSI's timetable buses drive a path against its direction where the trip's
    /// station links or track say so). Only timetable routes use them: they carry no traffic
    /// and no light. Returns how many were added.
    pub fn add_reverse_twins(&mut self, lanes: &[usize]) -> usize {
        let mut new = Vec::new();
        for &l in lanes {
            if !self.twinned.insert(l) {
                continue;
            }
            let o = &self.net.lanes[l];
            let pts: Vec<DVec3> = o.points.iter().rev().copied().collect();
            let mut t = omsi_sim::traffic::LaneBuilder::polyline(pts, o.kind, o.width);
            t.key = o.key;
            t.reversed = !o.reversed;
            t.speed_limit_kmh = o.speed_limit_kmh;
            t.source = o.source;
            t.offset = o.offset;
            t.name = o.name.clone();
            t.invisible = o.invisible;
            t.priority = o.priority;
            t.density = 0.0;
            t.no_cars = true;
            new.push(t);
        }
        let n = new.len();
        if n > 0 {
            let added = self.net.extend(new, 1.5);
            log::debug!("traffic: {n} lanes added for timetable routes that drive a one-way path the other way ({:?})", added);
        }
        n
    }

    /// A lane from the end of `a` to the start of `b`, for a timetable route that jumps a
    /// gap in the map (a road piece deleted after the timetable's tracks were recorded - a
    /// dozen such holes of 10 to 150 m on Novi Sad): a smooth curve with the two lanes'
    /// headings at its ends, used by the timetable only. None when the two do not line up.
    pub fn add_connector(&mut self, a: usize, b: usize) -> Option<usize> {
        let (la, lb) = (&self.net.lanes[a], &self.net.lanes[b]);
        let (p0, p1) = (la.end(), lb.start());
        let d = (p1 - p0).truncate();
        let len = d.length();
        if !(2.0..=150.0).contains(&len) || la.kind != lb.kind {
            return None;
        }
        let dir = |h: f32| {
            let r = (h as f64).to_radians();
            glam::DVec2::new(r.sin(), r.cos())
        };
        let (t0, t1) = (dir(la.end_heading()), dir(lb.start_heading()));
        let chord = d / len;
        // the lanes point along the gap or turn across it (a junction whose turning path
        // the map lost, 66 of them on Novi Sad), never a reversal
        if t0.dot(chord) < 0.3 || t1.dot(chord) < 0.3 || t0.dot(t1) < -0.2 {
            return None;
        }
        // (tangents as long as the gap make a straight gap a smooth S; across a turn they
        // would swing out past the corner)
        let len_t = len * if t0.dot(t1) > 0.9 { 1.0 } else { 0.6 };
        let n = ((len / 3.0).ceil() as usize).max(2);
        let pts: Vec<DVec3> = (0..=n)
            .map(|i| {
                let t = i as f64 / n as f64;
                let (h00, h10, h01, h11) = (2.0 * t * t * t - 3.0 * t * t + 1.0, t * t * t - 2.0 * t * t + t, -2.0 * t * t * t + 3.0 * t * t, t * t * t - t * t);
                let xy = p0.truncate() * h00 + t0 * len_t * h10 + p1.truncate() * h01 + t1 * len_t * h11;
                DVec3::new(xy.x, xy.y, p0.z + (p1.z - p0.z) * t)
            })
            .collect();
        let mut l = omsi_sim::traffic::LaneBuilder::polyline(pts, la.kind, la.width);
        l.speed_limit_kmh = la.speed_limit_kmh.min(lb.speed_limit_kmh);
        l.name = "(timetable connector)".into();
        l.density = 0.0;
        l.no_cars = true;
        let added = self.net.extend(vec![l], 1.5);
        log::debug!("traffic: a {len:.0} m connector lane {} from lane {a} to lane {b} for a timetable route", added.start);
        Some(added.start)
    }

    /// Hand timetable bus `ci` the next trip of its tour: its route from the lane it is on
    /// (`route[0]` is that lane, `s` where it is on it) and the trip's stops. It stays where
    /// it stands; a stop right there is served in place (its layover).
    pub fn reroute(&mut self, ci: usize, route: Vec<usize>, s: f32, stops: Vec<(usize, f32, f32, f64, i64, f32)>, layover: bool) {
        let net = &self.net;
        let car = &mut self.cars[ci];
        let lane = car.state.lane;
        car.state.route = route;
        car.state.route_index = 0;
        car.state.lane = lane;
        car.state.s = s;
        car.state.change = None;
        car.state.planned_next = None;
        car.state.ahead.clear();
        car.state.plan_next(net);
        car.gone = false;
        let stops = stops.into_iter().map(crate::bus_service::Stop::from_tuple).collect();
        match car.bus.as_mut() {
            Some(b) => b.restart(stops, layover),
            None => {
                let mut b = BusService::new(stops);
                b.layover = layover;
                car.bus = Some(Box::new(b));
            }
        }
    }

    /// Let timetable bus `ci` go at the end of its trip: it drives on as other traffic and
    /// is taken off as soon as nobody sees it.
    pub fn release(&mut self, ci: usize) {
        let net = &self.net;
        let car = &mut self.cars[ci];
        let st = &mut car.state;
        st.route.clear();
        st.route_index = 0;
        st.planned_next = None;
        st.ahead.clear();
        st.plan_next(net);
        if let Some(b) = car.bus.as_mut() {
            b.restart(Vec::new(), false);
        }
        car.gone = true;
        car.vehicle.host.schedule_active = 0.0;
        car.vehicle.host.tt_line.clear();
        car.vehicle.host.tt_stops.clear();
        car.vehicle.host.tt_stop_ids.clear();
        car.vehicle.host.tt_busstop_index = -1;
        car.vehicle.host.tt_terminus_index = -1;
        car.vehicle.host.tt_delay = 0.0;
        car.vehicle.set_var("schedule_active", 0.0);
    }

    /// Take all random AI cars off the road now, keeping timetable buses. Returns how many
    /// vehicles were removed. The configured target is unchanged, so random traffic can
    /// populate the roads again normally.
    pub fn clear_random(&mut self, world: &World, renderer: &Renderer, scene: &mut Scene) -> usize {
        let ids: Vec<u64> = self.cars.iter().filter(|c| !c.is_bus()).map(|c| c.id).collect();
        let removed = ids.len();
        for id in ids {
            self.remove_car(world, renderer, scene, id);
        }
        removed
    }

    /// The AI on the roads: (cars, buses, cars asleep far from everybody, parked cars).
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let buses = self.cars.iter().filter(|c| c.is_bus()).count();
        (self.cars.len() - buses, buses, self.dormant.len(), self.parked.values().map(Vec::len).sum())
    }

    /// Take a car off the road now (the player took over its tour).
    pub fn remove_car(
        &mut self,
        world: &World,
        renderer: &Renderer,
        scene: &mut Scene,
        id: u64,
    ) -> bool {
        let Some(i) = self.cars.iter().position(|c| c.id == id) else {
            return false;
        };
        let c = self.cars.swap_remove(i);
        self.orphan_sounds.extend(c.sounds);
        for r in std::iter::once(c.render).chain(c.trailer_renders) {
            world.release_vehicle(renderer, scene, r);
        }
        true
    }

    /// Obstacle boxes of all AI vehicles (for the player's collisions), with the rear
    /// sections of articulated buses and the trailers.
    pub fn boxes(&self, near: DVec3, radius: f64) -> Vec<omsi_sim::collision::Obb> {
        self.cars
            .iter()
            .filter(|c| (c.vehicle.position - near).length() < radius)
            .flat_map(|c| {
                let bb = c
                    .vehicle
                    .ty
                    .def
                    .bounding_box
                    .unwrap_or([2.0, 4.5, 1.6, 0.0, 0.0, 0.8]);
                // moving, and with a mass of its own: a car that runs into the bus is no
                // bulldozer
                let h = c.vehicle.heading.to_radians();
                let v = glam::DVec2::new(h.sin(), h.cos()) * c.state.speed as f64;
                let (mass, id) = (c.vehicle.physics.mass_kg, c.id);
                let rear = c.vehicle.trailers.iter().filter_map(move |t| {
                    t.ty.def.bounding_box.map(|bb| {
                        omsi_sim::collision::Obb::from_box(bb, t.position, t.body_heading())
                            .moving(v, mass, id)
                    })
                });
                std::iter::once(
                    omsi_sim::collision::Obb::from_box(bb, c.vehicle.position, c.vehicle.body_heading())
                        .moving(v, mass, id),
                )
                .chain(rear)
            })
            .collect()
    }

    /// Position and heading of a car by id (None once it is gone).
    pub fn car_pose(&self, id: u64) -> Option<(DVec3, f64)> {
        self.cars
            .iter()
            .find(|c| c.id == id)
            .map(|c| (c.vehicle.position, c.vehicle.heading))
    }

    /// Tell a scheduled bus's script who wants in or out (`PAX_Entry<i>_Req`,
    /// `PAX_Exit<i>_Req`) and who stands in its doorways (`_Busy`): the stock AI door
    /// scripts open the rear doors only for a stop request, which comes from the exit
    /// requests.
    pub fn set_pax_requests(&mut self, id: u64, doors: &crate::humans::DoorWants) {
        if let Some(c) = self.cars.iter_mut().find(|c| c.id == id) {
            crate::humans::Humans::write_door_requests(&mut c.vehicle, doors);
        }
    }

    /// Keep a scheduled bus at its stop for at least `secs` more with the doors open:
    /// passengers are still queueing at a door or stepping in.
    /// The passengers' wishes for the timetable buses' next stops (see `stop_wishes`).
    pub fn set_stop_wishes(&mut self, alighting: hashbrown::HashSet<u64>, waiting: hashbrown::HashSet<i64>) {
        self.stop_wishes = Some((alighting, waiting));
    }

    pub fn hold_boarding(&mut self, id: u64, stop: Option<i64>, secs: f32) {
        if let Some(c) = self.cars.iter_mut().find(|c| c.id == id) {
            if let Some(b) = c.bus.as_mut() {
                b.hold(stop, secs);
            }
        }
    }
}
