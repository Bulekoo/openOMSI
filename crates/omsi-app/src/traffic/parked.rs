//! Parked cars: beside which lanes they stand, cars parking in a free space and parked
//! cars pulling out.

use super::*;
use crate::scene::World;
use omsi_render::{Renderer, Scene};

impl Traffic {
    /// Put parked cars onto the lanes they stand in or beside: (distance along the lane,
    /// signed lateral offset, + = right). `cars` are the ones the tiles placed since the last
    /// call; the cars no lane was found for before are tried again where the lanes `added`
    /// just came in.
    pub(super) fn sort_parked(&mut self, cars: Vec<(DVec3, f64)>, added: std::ops::Range<usize>) {
        let mut todo: Vec<DVec3> = Vec::new();
        if !added.is_empty() && !self.parked_waiting.is_empty() {
            let cells: hashbrown::HashSet<(i32, i32)> = added
                .clone()
                .flat_map(|i| Network::lane_cells(&self.net.lanes[i]))
                .collect();
            let near = |p: &DVec3| {
                let (cx, cy) = Network::grid_cell(*p);
                (-1..=1).any(|dx| (-1..=1).any(|dy| cells.contains(&(cx + dx, cy + dy))))
            };
            let (retry, keep): (Vec<DVec3>, Vec<DVec3>) = std::mem::take(&mut self.parked_waiting)
                .into_iter()
                .partition(|p| near(p));
            self.parked_waiting = keep;
            todo = retry;
        }
        let new_cars = cars.len();
        todo.extend(cars.into_iter().map(|(p, _heading)| p));
        if todo.is_empty() {
            return;
        }
        let mut on_lanes = 0usize;
        for p in todo {
            // a car beside a lane is within a few metres of it: the lanes of the cells
            // around it are enough, and a car in a car park finds none
            let beside = self
                .net
                .nearest_lane_near(p, LaneKind::Street)
                .filter(|(l, _, d)| *d <= (self.net.lanes[*l].width * 0.5).max(1.5) as f64 + 1.6);
            let Some((l, s, _)) = beside else {
                self.parked_waiting.push(p);
                continue;
            };
            let (q, h) = self.net.lanes[l].at(s);
            let hr = (h as f64).to_radians();
            let right = DVec3::new(hr.cos(), -hr.sin(), 0.0);
            let lat = (p - q).dot(right) as f32;
            if lat.abs() < 0.9 && omsi_cfg::env::var_os("OMSI_DEBUG_TRAFFIC").is_some() {
                let lane = &self.net.lanes[l];
                log::info!("parked car at ({:.1}, {:.1}) stands in lane {l} ({} {:?}, width {:.1}, heading {:.0} there, s {s:.1} of {:.1}, {lat:+.2} m to the side)", p.x, p.y, lane.name, lane.key, lane.width, h, lane.length());
            }
            self.parked.entry(l).or_default().push((s, lat));
            on_lanes += 1;
        }
        if omsi_cfg::env::var_os("OMSI_DEBUG_TRAFFIC").is_some() {
            log::info!("traffic: {new_cars} parked cars placed, {on_lanes} more stand in or beside a lane ({} in all, {} not beside one)", self.parked.values().map(|v| v.len()).sum::<usize>(), self.parked_waiting.len());
        }
    }

    /// Now and then a car near the player parks: a space at the kerb that a parked car has
    /// left is taken by a car of the same kind driving up the lane beside it - it indicates,
    /// slows down, stops beside the space, moves over into it and stands there as the
    /// parked car it was. Called with every population pass (about every two seconds).
    pub fn park_in(&mut self, world: &World, center: DVec3) {
        let forced = omsi_cfg::env::var("OMSI_PARK_IN").ok().and_then(|v| v.parse::<f64>().ok());
        if self.rand_f() >= forced.unwrap_or(0.04) {
            return;
        }
        let debug = omsi_cfg::env::var_os("OMSI_DEBUG_TRAFFIC").is_some() || forced.is_some();
        let taken: hashbrown::HashSet<i64> = self.cars.iter().filter_map(|c| c.park.map(|p| p.key)).collect();
        let mut spots = world.free_parking();
        spots.retain(|(k, p)| !taken.contains(k) && (30.0..260.0).contains(&(p.pos - center).truncate().length()));
        spots.sort_by_key(|s| s.0);
        let mut why: Vec<String> = Vec::new();
        while !spots.is_empty() {
            let (key, p) = spots.swap_remove(self.rand() as usize % spots.len());
            let Some((l, s, _)) = self.net.nearest_lane_near(p.pos, LaneKind::Street) else { continue };
            let lane = &self.net.lanes[l];
            if lane.no_cars || s < 8.0 || s > lane.length() - 4.0 {
                continue;
            }
            let (q, h) = lane.at(s);
            let hr = (h as f64).to_radians();
            let lat = (p.pos - q).dot(DVec3::new(hr.cos(), -hr.sin(), 0.0)) as f32;
            let mut dh = (p.heading - h as f64).rem_euclid(360.0);
            if dh > 180.0 {
                dh -= 360.0;
            }
            // beside the lane on the right and in line with it (a space across the kerb or in
            // a row is not driven into)
            if !(1.2..4.2).contains(&lat) || dh.abs() > 12.0 {
                why.push(format!("space {key}: {lat:+.1} m beside lane {l}, {dh:+.0} deg"));
                continue;
            }
            let folder = p.sco.parent().map(|d| d.to_string_lossy().to_lowercase());
            // a car of that kind coming up the lane, far enough off to slow down gently
            let mut best: Option<(usize, f32)> = None;
            for (i, c) in self.cars.iter().enumerate() {
                if c.is_bus() || c.gone || c.park.is_some() || c.passing.is_some() || c.state.change.is_some() || c.pull_out > 0.0 {
                    continue;
                }
                if c.vehicle.ty.def.path.parent().map(|d| d.to_string_lossy().to_lowercase()) != folder {
                    continue;
                }
                if c.state.speed > 15.0 || c.state.lateral.abs() > 0.2 {
                    continue;
                }
                let Some(&(_, dl)) = self.way_lanes(&c.state, 160.0).iter().find(|w| w.0 == l) else { continue };
                let d = dl + s;
                let need = c.state.speed * c.state.speed / 2.0 + 20.0;
                if d > need && d < 150.0 && best.map(|b| d < b.1).unwrap_or(true) {
                    best = Some((i, d));
                }
            }
            let Some((i, d)) = best else {
                why.push(format!("space {key}: no car of its kind coming up lane {l}"));
                continue;
            };
            let car = &mut self.cars[i];
            car.park = Some(ParkPlan { key, lane: l, s, lat, ramped: false, done: false });
            // (it parks: no more route to plan than to here)
            car.gone = false;
            if debug {
                log::info!("car {} parks in the space of parked car {key} ({:.0} m ahead, {lat:.1} m right of lane {l})", car.id, d);
            }
            return;
        }
        if debug && !why.is_empty() {
            log::info!("park-in: none this time ({})", why.join("; "));
        }
    }

    /// Now and then a car parked at the kerb near the player drives off: the parked object
    /// goes (its space stays empty) and the AI car of the same folder takes its place, in
    /// the parking position, indicating, and pulls out into its lane once the road behind
    /// it is clear. Called with every population pass (about every two seconds).
    pub fn pull_out_parked(&mut self, world: &World, renderer: &Renderer, scene: &mut Scene, center: DVec3) {
        let forced = omsi_cfg::env::var("OMSI_PARKED_PULL_OUT").ok().and_then(|v| v.parse::<f64>().ok());
        // about one car a minute
        if self.rand_f() >= forced.unwrap_or(0.035) {
            return;
        }
        let debug = omsi_cfg::env::var_os("OMSI_DEBUG_TRAFFIC").is_some() || forced.is_some();
        let mut candidates: Vec<(i64, crate::scene::ParkedObject)> = world
            .parked_objects
            .lock()
            .iter()
            .filter(|(_, p)| {
                let d = (p.pos - center).truncate().length();
                (25.0..180.0).contains(&d)
            })
            .map(|(k, p)| (*k, p.clone()))
            .collect();
        candidates.sort_by_key(|c| c.0);
        if debug {
            log::info!("parked pull-out: {} parked cars in range ({} loaded)", candidates.len(), world.parked_objects.lock().len());
        }
        let mut why: Vec<String> = Vec::new();
        let mut tried = 0;
        while !candidates.is_empty() && tried < 8 {
            tried += 1;
            let (key, p) = candidates.swap_remove(self.rand() as usize % candidates.len());
            // the AI car of the same folder (a parked Golf is `parked_vw_golf_2.sco` next to
            // `ai_vw_golf_2.bus`)
            let folder = p.sco.parent().map(|d| d.to_string_lossy().to_lowercase());
            let Some(ty) = self
                .types
                .iter()
                .filter(|t| t.2 == LaneKind::Street)
                .find(|t| t.0.def.path.parent().map(|d| d.to_string_lossy().to_lowercase()) == folder)
                .map(|t| t.0.clone())
            else {
                why.push(format!("no AI car in {folder:?}"));
                continue;
            };
            let Some((l, s, _)) = self.net.nearest_lane_near(p.pos, LaneKind::Street) else {
                why.push("no lane".into());
                continue;
            };
            let lane = &self.net.lanes[l];
            if lane.no_cars || s < 4.0 || s > lane.length() - 4.0 {
                continue;
            }
            let (q, h) = lane.at(s);
            let hr = (h as f64).to_radians();
            let right = DVec3::new(hr.cos(), -hr.sin(), 0.0);
            let lat = (p.pos - q).dot(right) as f32;
            // beside the lane on the right, facing its way (one parked in the lane itself stands
            // bumper to bumper in a row it cannot steer out of)
            let mut dh = (p.heading - h as f64).rem_euclid(360.0);
            if dh > 180.0 {
                dh -= 360.0;
            }
            if !(0.8..4.5).contains(&lat) || dh.abs() > 30.0 {
                why.push(format!("{lat:+.1} m beside lane {l}, {dh:+.0} deg to it"));
                continue;
            }
            // nobody close by on the road
            if self.cars.iter().any(|c| (c.vehicle.position - p.pos).length() < 30.0) || !self.spawn_clear(&ty, q, h as f64) {
                why.push("road not clear".into());
                continue;
            }
            if world.depart_parked(renderer, scene, key).is_none() {
                continue;
            }
            if let Some(list) = self.parked.get_mut(&l) {
                if let Some(j) = (0..list.len()).min_by(|&a, &b| (list[a].0 - s).abs().total_cmp(&(list[b].0 - s).abs())) {
                    if (list[j].0 - s).abs() < 3.0 {
                        list.swap_remove(j);
                    }
                }
            }
            let seed = self.rand();
            let id = self.create_car(world, renderer, scene, center, LaneKind::Street, l, s, ty.clone(), seed, None, None, Some(0.0), None);
            let net = &self.net;
            if let Some(car) = self.cars.iter_mut().find(|c| c.id == id) {
                car.state.lateral = lat;
                car.state.lateral_target = 0.0;
                car.state.lateral_ramp = (lat, 0.0, car.state.odometer, (lat * 6.0).clamp(8.0, 16.0));
                car.body = place_body(net, &car.state, &mut car.vehicle, MotionKind::Road);
                car.pull_out = 2.0 + (seed % 1000) as f32 / 400.0;
                car.state.blinker = 1;
            }
            if debug {
                log::info!("parked car {key} ({}) at ({:.1}, {:.1}) pulls out as car {id}, {lat:.1} m right of lane {l}", ty.def.path.display(), p.pos.x, p.pos.y);
            }
            return;
        }
        if debug && !why.is_empty() {
            log::info!("parked pull-out: none this time ({})", why.join("; "));
        }
    }
}

/// A parked car uses the same approximate body as the driving obstacle check:
/// half length 2.3 m, half width 0.9 m, and 0.15 m lateral clearance. A car well
/// off the target lane must not prevent a lane change on a narrow street.
pub(super) fn parked_lane_clear(
    parked: &[(f32, f32)],
    s: f32,
    back: f32,
    ahead: f32,
    half_width: f32,
) -> bool {
    !parked.iter().any(|&(at, lat)| {
        lat.abs() < half_width + 0.9 + 0.15 && at + 2.3 >= s - back && at - 2.3 <= s + ahead
    })
}
