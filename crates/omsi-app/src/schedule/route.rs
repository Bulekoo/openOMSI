//! A trip's route on the lanes of the network: its steps, the slots the loaded tiles have for
//! them, and its stops placed on it.

use super::*;

/// The bus stands next to the kerb: the pole's offset less half a bus width and a gap; only
/// where the pole is clearly off the lane (a bay).
///
/// A pole further off than a bay's width stands behind the pavement or the verge (the
/// stop objects of many maps are placed there): the bus stays in its lane at the kerb then.
/// Taken as a bay up to 4 m wide, the bus pulled out over the kerb onto the grass.
///
/// Not at all, now: a timetable bus stays on its path at the stop, as OMSI's do (a
/// map's bus bay is a spline of its own that the route runs through). The pole's offset
/// says nothing about where the kerb is - most stand behind the pavement - and a bus
/// moved 1.6 m to the right of its lane drove along with its right wheels on the pavement.
/// Stops moved `shift` metres back along `route` (the lanes the stops' route indices less
/// `base` count in): where the vehicle's origin comes to rest (`bus_service::stop_shift`).
/// One that comes to lie before the route's first lane keeps a distance below zero on it.
pub(super) fn shift_stops(net: &Network, route: &[usize], base: usize, stops: &mut [(usize, f32, f32, f64, i64, f32)], shift: f32) {
    if shift.abs() < 1e-3 {
        return;
    }
    for st in stops.iter_mut() {
        let (mut k, mut ss) = (st.0.saturating_sub(base), st.1 - shift);
        while ss < 0.0 && k > 0 && k <= route.len() - 1 {
            k -= 1;
            ss += net.lanes[route[k]].length();
        }
        while ss > 0.0 && k + 1 < route.len() && ss > net.lanes[route[k]].length() {
            ss -= net.lanes[route[k]].length();
            k += 1;
        }
        st.0 = base + k;
        st.1 = ss;
    }
}

pub(super) fn bay_offset(lat: f32) -> f32 {
    lat
}

/// Where a timetable bus stands across its lane at a stop, as Omsi.exe puts it
/// (0x7dac5e..0x7dae81): its kerb-side flank 0.3 m past the `[busstop]` box's centre -
/// `lat` less its `[boundingbox]` lateral centre and half width plus 0.3 on the right (the other way round
/// where traffic keeps left), from the box's offset `lat` off the path (right positive);
/// a railway vehicle keeps to its track. OMSI clamps it only to the room beside other
/// vehicles, not to a kerb: the bus pulls into the bay whether or not a path leads there
/// (#241). (openOMSI kept it on its path before - a map whose box stood behind the
/// pavement had its buses on the pavement - but OMSI does the same there.)
pub(super) fn bay_for(lat: f32, ty: &omsi_sim::VehicleType, rail: bool, left_hand: bool, side: f32) -> f32 {
    if rail || !lat.is_finite() {
        return 0.0;
    }
    let bb = ty.def.bounding_box.unwrap_or([2.5, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let (hw, centre) = (bb[0] * 0.5, bb[3]);
    // Platform side is independent of traffic hand. With boarding on both sides,
    // align the flank facing this stop's box rather than assuming a right-hand kerb.
    let left = if side == 2.0 { lat < 0.0 } else { left_hand != (side == 1.0) };
    if left {
        lat - centre + hw - 0.3
    } else {
        lat - centre - hw + 0.3
    }
}

/// The stops' raw box offsets (see `bay_offset`) made the vehicle's bay offsets, and the
/// stops moved to where its origin comes to rest (`shift_stops`).
pub(super) fn place_stops(net: &Network, route: &[usize], base: usize, stops: &mut [(usize, f32, f32, f64, i64, f32)], ty: &omsi_sim::VehicleType, rail: bool) {
    for st in stops.iter_mut() {
        st.2 = bay_for(st.2, ty, rail, net.left_hand, st.5);
    }
    shift_stops(net, route, base, stops, crate::bus_service::stop_shift(ty, rail));
}

/// Where on `route` the bus stop at `pos` is: (route index, distance along that lane, lateral
/// offset); None when it is further than `reach` from the route.
/// Where the stop at `pos` lies on `route`, not before route index `from` (the stops come
/// in the trip's order): on the side of the road it stands, see
/// `Network::project_stop_on_route`.
pub(super) fn project_stop(
    net: &Network,
    route: &[usize],
    pos: glam::DVec3,
    reach: Option<f64>,
    from: usize,
    side: f32,
    on: StopRoute,
) -> Option<(usize, f32, f32)> {
    match on {
        StopRoute::Nearest => net.project_stop_on_route_side(route, pos, reach, from, side as u8),
        StopRoute::Outside => None,
        StopRoute::Track(ri) => {
            let lane = *route.get(ri)?;
            let (_, s, lat) = net.project_on_route_lateral(&[lane], pos)?;
            let point = net.lanes[lane].at(s).0;
            if reach.is_some_and(|r| (point - pos).truncate().length() > r) {
                return None;
            }
            Some((ri, s, lat))
        }
    }
}

impl Schedule {
    /// The steps of a trip's route: from the trip's own track when it has one (trains,
    /// ferries, planes), else from the station links between its stops. The flag says it is
    /// a track.
    ///
    /// The track is the one the trip's `[trip]` block names on its first line (Novi Sad's
    /// trip "1 Klisa-Liman I" runs track "1_Klisa-Liman1"; the stock trains name tracks of
    /// their own name), else the one named like the trip.
    pub(super) fn steps_of(&self, track_name: &str, stations: &[i64]) -> (Vec<Step>, bool) {
        let track_name = self
            .data
            .trip(track_name)
            .map(|t| t.display_name.trim())
            .filter(|n| !n.is_empty())
            .unwrap_or(track_name);
        let key = |id: f64, path: f64, tile_index: f64| {
            self.tile_coords
                .get(tile_index as usize)
                .map(|&tile| LaneKey {
                    tile,
                    id: id as i64,
                    path: path as u16,
                })
        };
        let mut steps: Vec<Step> = Vec::new();
        if let Some(track) = self.data.tracks.iter().find(|t| {
            t.path
                .file_stem()
                .map(|s| s.to_string_lossy().eq_ignore_ascii_case(track_name))
                .unwrap_or(false)
        }) {
            steps.extend(
                track
                    .entries
                    .iter()
                    .filter(|e| e.values.len() >= 5)
                    .map(|e| Step {
                        key: key(e.values[0], e.values[1], e.values[2]),
                        leg: 0,
                        length: e.values[4],
                    }),
            );
            return (steps, true);
        }
        for (leg, w) in stations.windows(2).enumerate() {
            match self
                .data
                .stn_links
                .iter()
                .find(|l| l.from_id == w[0] && l.to_id == w[1])
            {
                Some(link) => {
                    for e in &link.entries {
                        let k = key(e.values[0], e.values[1], e.values[2]);
                        // consecutive links repeat the shared lane
                        if steps.last().map(|s| s.key == k).unwrap_or(false) {
                            continue;
                        }
                        steps.push(Step {
                            key: k,
                            leg,
                            length: e.values[3],
                        });
                    }
                }
                None => log::debug!("trip {track_name}: no station link {} -> {}", w[0], w[1]),
            }
        }
        (steps, false)
    }

    /// One-way paths that a route drives the other way - their end lies where the path
    /// before it ends, their start where the next one begins - get a lane that way
    /// (`Traffic::add_reverse_twins`). OMSI's timetable buses follow their station links
    /// and tracks whichever way a path runs: Spandau's line to Kladow and a dozen Novi Sad
    /// tracks run over invisible one-way helper streets backwards, and the bus drove them
    /// forwards, against its route, and jumped back at their end.
    pub(super) fn add_twins(traffic: &mut Traffic, steps: &[Step]) {
        let net = &traffic.net;
        let cands: Vec<Option<&Vec<usize>>> = steps
            .iter()
            .map(|st| st.key.and_then(|k| net.by_key.get(&k)).filter(|c| !c.is_empty()))
            .collect();
        let ends = |c: &Vec<usize>| -> Vec<glam::DVec3> {
            c.iter().flat_map(|&l| [net.lanes[l].start(), net.lanes[l].end()]).collect()
        };
        let near = |p: glam::DVec3, pts: &[glam::DVec3]| {
            pts.iter().map(|q| (*q - p).truncate().length()).fold(f64::MAX, f64::min)
        };
        let mut want = Vec::new();
        for (i, c) in cands.iter().enumerate() {
            // a path that runs both ways has its lanes already
            let Some(c) = c.filter(|c| c.len() == 1) else { continue };
            let l = &net.lanes[c[0]];
            let prev = i.checked_sub(1).and_then(|k| cands[k]).map(ends);
            let next = cands.get(i + 1).copied().flatten().map(ends);
            if prev.is_none() && next.is_none() {
                continue;
            }
            let score = |a: glam::DVec3, b: glam::DVec3| {
                prev.as_ref().map(|p| near(a, p)).unwrap_or(0.0)
                    + next.as_ref().map(|n| near(b, n)).unwrap_or(0.0)
            };
            let (fwd, bwd) = (score(l.start(), l.end()), score(l.end(), l.start()));
            if bwd + 3.0 < fwd && bwd < 6.0 {
                want.push(c[0]);
            }
        }
        if !want.is_empty() {
            traffic.add_reverse_twins(&want);
        }
    }

    /// Where a route's lanes do not join and the network has no way between them either,
    /// a connector lane across the gap (`Traffic::add_connector`), so that `bridge_gaps`
    /// finds a way to drive.
    pub(super) fn add_connectors(traffic: &mut Traffic, lanes: &[usize]) {
        let net = &traffic.net;
        let holes: Vec<(usize, usize)> = lanes
            .windows(2)
            .filter(|w| !joins(net, w[0], w[1]))
            .filter(|w| {
                let gap = (net.lanes[w[1]].start() - net.lanes[w[0]].end()).truncate().length();
                way_between(net, w[0], w[1], (gap * 2.5 + 60.0) as f32).is_none()
            })
            .map(|w| (w[0], w[1]))
            .collect();
        for (a, b) in holes {
            traffic.add_connector(a, b);
        }
    }

    /// The steps as the loaded network has them, each lane's direction chosen so that it
    /// follows the lane before it (`prev` for the first) and leads into the one after it.
    /// Taking whichever direction came first - as the first step used to, with nothing
    /// before it - sent the route (and the navigator) the wrong way along a two-way street.
    pub(super) fn slots(
        &self,
        world: &World,
        traffic: &Traffic,
        steps: &[Step],
        prev: Option<usize>,
    ) -> Vec<Slot> {
        let net = &traffic.net;
        // every step's candidate lanes (both directions of a two-way path)
        let cands: Vec<Result<&Vec<usize>, Slot>> = steps
            .iter()
            .map(|st| {
                let Some(key) = st.key else {
                    return Err(Slot::Absent);
                };
                match net.by_key.get(&key) {
                    Some(c) if !c.is_empty() => Ok(c),
                    _ if !traffic.lane_tiles.contains(&key.tile) && world.has_tile(key.tile) => {
                        Err(Slot::Waiting)
                    }
                    _ => Err(Slot::Absent),
                }
            })
            .collect();
        let mut out = Vec::with_capacity(steps.len());
        let mut last = prev;
        for (i, c) in cands.iter().enumerate() {
            let c = match c {
                Ok(c) => *c,
                Err(slot) => {
                    if *slot == Slot::Waiting {
                        last = None;
                    }
                    out.push(*slot);
                    continue;
                }
            };
            // the next lanes the route has (not across a gap)
            let next = cands[i + 1..].iter().find_map(|x| match x {
                Ok(n) => Some(Some(*n)),
                Err(Slot::Waiting) => Some(None),
                Err(_) => None,
            });
            let next = next.flatten();
            let score = |l: usize| -> f64 {
                let mut s = 0.0;
                if let Some(prev) = last {
                    s += (net.lanes[l].start() - net.lanes[prev].end()).length();
                }
                if let Some(next) = next {
                    let end = net.lanes[l].end();
                    s += next
                        .iter()
                        .map(|&n| (net.lanes[n].start() - end).length())
                        .fold(f64::MAX, f64::min);
                }
                s
            };
            let best = c
                .iter()
                .copied()
                .min_by(|a, b| score(*a).total_cmp(&score(*b)))
                .unwrap();
            out.push(Slot::Lane(best));
            last = Some(best);
        }
        skip_detours(net, &mut out);
        if omsi_cfg::flags::OMSI_DEBUG_ROUTES.is_set() {
            // where consecutive lanes of the route do not join (a gap, or a change within the
            // same spline, which is a lane change)
            let lanes: Vec<usize> = out
                .iter()
                .filter_map(|s| {
                    if let Slot::Lane(l) = s {
                        Some(*l)
                    } else {
                        None
                    }
                })
                .collect();
            for (k, w) in lanes.windows(2).enumerate() {
                let (a, b) = (&net.lanes[w[0]], &net.lanes[w[1]]);
                let gap = (b.start() - a.end()).truncate().length();
                if gap > 2.0
                    || a.key.map(|k| (k.tile, k.id, k.path))
                    == b.key.map(|k| (k.tile, k.id, k.path))
                {
                    log::info!("route: step {k}: lane {} {:?} rev {} -> lane {} {:?} rev {}: gap {gap:.1} m, linked {}, lane change {}", w[0], a.key, a.reversed, w[1], b.key, b.reversed, a.next.contains(&w[1]), net.parallel(w[0], w[1]));
                }
            }
        }
        let waiting = out.iter().filter(|s| **s == Slot::Waiting).count();
        let absent = out.iter().filter(|s| **s == Slot::Absent).count();
        if absent > 0 || omsi_cfg::flags::OMSI_DEBUG_TRAFFIC.is_set() {
            log::debug!("route: {} of {} steps on loaded lanes, {waiting} on tiles still to come, {absent} not in the map", out.len() - waiting - absent, out.len());
        }
        out
    }

    /// The lanes a trip runs on (for the navigator): its track, else the station links
    /// between its stops, as far as the tiles have brought them - and whether that is all.
    pub fn trip_route(
        &self,
        world: &World,
        traffic: &Traffic,
        trip_name: &str,
    ) -> (Vec<usize>, bool) {
        let Some(trip) = self
            .data
            .trips
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(trip_name))
        else {
            return (Vec::new(), true);
        };
        let slots = self.slots(
            world,
            traffic,
            &self.steps_of(trip_name, &trip_stations(trip)).0,
            None,
        );
        let complete = !slots.contains(&Slot::Waiting);
        (
            slots
                .into_iter()
                .filter_map(|s| if let Slot::Lane(l) = s { Some(l) } else { None })
                .collect(),
            complete,
        )
    }

    /// The lanes a trip runs on in `net` - the navigator's network of the whole map, which
    /// has every tile's lanes whether loaded or not - chosen as `slots` chooses them (of a
    /// two-way path the direction that joins the lanes before and after).
    pub fn trip_route_in(&self, net: &omsi_sim::traffic::Network, trip_name: &str) -> Vec<usize> {
        let Some(trip) = self.data.trips.iter().find(|x| x.name.eq_ignore_ascii_case(trip_name)) else {
            return Vec::new();
        };
        let (steps, _) = self.steps_of(trip_name, &trip_stations(trip));
        let cands: Vec<Option<&Vec<usize>>> = steps.iter().map(|st| st.key.and_then(|k| net.by_key.get(&k)).filter(|c| !c.is_empty())).collect();
        let mut out: Vec<Slot> = Vec::with_capacity(steps.len());
        let mut last: Option<usize> = None;
        for (i, c) in cands.iter().enumerate() {
            let Some(c) = c else {
                out.push(Slot::Absent);
                continue;
            };
            let next = cands[i + 1..].iter().find_map(|x| *x);
            let score = |l: usize| -> f64 {
                let mut s = 0.0;
                if let Some(prev) = last {
                    s += (net.lanes[l].start() - net.lanes[prev].end()).length();
                }
                if let Some(next) = next {
                    let end = net.lanes[l].end();
                    s += next.iter().map(|&n| (net.lanes[n].start() - end).length()).fold(f64::MAX, f64::min);
                }
                s
            };
            let best = c.iter().copied().min_by(|a, b| score(*a).total_cmp(&score(*b))).unwrap();
            out.push(Slot::Lane(best));
            last = Some(best);
        }
        skip_detours(net, &mut out);
        out.into_iter().filter_map(|s| if let Slot::Lane(l) = s { Some(l) } else { None }).collect()
    }

    /// `OMSI_CHECK_TRIPS=1`: build the route of every trip on the loaded lanes and say where
    /// consecutive lanes do not join - a gap the bus would jump, or a lane taken the wrong
    /// way round (its end, not its start, lies where the lane before ends), which sends a
    /// bus into the oncoming traffic. Only trips whose route is wholly loaded are judged.
    pub fn check_routes(&self, world: &World, traffic: &mut Traffic) {
        for trip in &self.data.trips {
            let (steps, _) = self.steps_of(&trip.name, &trip_stations(trip));
            Self::add_twins(traffic, &steps);
            let lanes: Vec<usize> = self
                .slots(world, traffic, &steps, None)
                .iter()
                .filter_map(|s| if let Slot::Lane(l) = s { Some(*l) } else { None })
                .collect();
            Self::add_connectors(traffic, &lanes);
        }
        let traffic = &*traffic;
        let net = &traffic.net;
        let (mut trips, mut joints, mut linked, mut changes, mut gaps, mut wrong, mut partial) =
            (0, 0, 0, 0, 0, 0, 0);
        let mut bad_length = 0;
        for trip in &self.data.trips {
            let stations = trip_stations(trip);
            let (steps, _) = self.steps_of(&trip.name, &stations);
            let slots = self.slots(world, traffic, &steps, None);
            if slots.contains(&Slot::Waiting) || steps.is_empty() {
                if partial < 5 {
                    let k = slots.iter().position(|s| *s == Slot::Waiting).unwrap_or(0);
                    log::info!(
                        "check trips: {}: {} steps, {} on tiles not loaded, first {:?} (tile loaded {}, in the map {})",
                        trip.name,
                        steps.len(),
                        slots.iter().filter(|s| **s == Slot::Waiting).count(),
                        steps.get(k).and_then(|s| s.key),
                        steps.get(k).and_then(|s| s.key).map(|key| traffic.lane_tiles.contains(&key.tile)).unwrap_or(false),
                        steps.get(k).and_then(|s| s.key).map(|key| world.has_tile(key.tile)).unwrap_or(false),
                    );
                }
                partial += 1;
                continue;
            }
            trips += 1;
            // `OMSI_CHECK_TRIPS=<trip name>`: every step of that trip
            if omsi_cfg::flags::OMSI_CHECK_TRIPS.var().map(|v| v.eq_ignore_ascii_case(&trip.name)).unwrap_or(false) {
                for (k, (st, sl)) in steps.iter().zip(&slots).enumerate() {
                    let cands: Vec<String> = st
                        .key
                        .and_then(|key| net.by_key.get(&key))
                        .map(|c| {
                            c.iter()
                                .map(|&l| {
                                    let x = &net.lanes[l];
                                    format!("{l}{} ({:.1},{:.1})->({:.1},{:.1}) {:.1} m", if x.reversed { "r" } else { "" }, x.start().x, x.start().y, x.end().x, x.end().y, x.length())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    log::info!("check trips: {} step {k} leg {} {:?} ({:.1} m): {:?} of {:?}", trip.name, st.leg, st.key, st.length, sl, cands);
                }
            }
            // the lane a step names should be as long as the file says its path is
            for (st, sl) in steps.iter().zip(&slots) {
                if let Slot::Lane(l) = sl {
                    let len = net.lanes[*l].length() as f64;
                    if st.length > 0.5 && (len - st.length).abs() > 1.0 + 0.05 * st.length {
                        if bad_length < 12 {
                            log::info!(
                                "check trips: {}: lane {} {:?} is {len:.1} m long, the file says {:.1} m",
                                trip.name, l, net.lanes[*l].key, st.length
                            );
                        }
                        bad_length += 1;
                    }
                }
            }
            let lanes: Vec<usize> = slots
                .iter()
                .filter_map(|s| if let Slot::Lane(l) = s { Some(*l) } else { None })
                .collect();
            let lanes = bridge_gaps(net, &lanes).0;
            let mut shown = 0;
            for (k, w) in lanes.windows(2).enumerate() {
                let (a, b) = (&net.lanes[w[0]], &net.lanes[w[1]]);
                joints += 1;
                if a.next.contains(&w[1]) {
                    linked += 1;
                    continue;
                }
                if net.parallel(w[0], w[1]) {
                    changes += 1;
                    continue;
                }
                let gap = (b.start() - a.end()).truncate().length();
                let backwards = (b.end() - a.end()).truncate().length();
                let is_wrong = backwards + 1.0 < gap && backwards < 3.0;
                if is_wrong {
                    wrong += 1;
                } else if gap > 2.0 {
                    gaps += 1;
                } else {
                    linked += 1;
                    continue;
                }
                if shown < 6 {
                    shown += 1;
                    log::info!(
                        "check trips: {}: step {k}: lane {} {:?} rev {} -> {} {:?} rev {}: {} (gap {gap:.1} m, to its end {backwards:.1} m) at ({:.0}, {:.0})",
                        trip.name, w[0], a.key, a.reversed, w[1], b.key, b.reversed,
                        if is_wrong { "WRONG WAY" } else { "gap" }, a.end().x, a.end().y
                    );
                }
            }
        }
        log::info!("check trips: {trips} trips on loaded lanes ({partial} not wholly loaded): {joints} joints, {linked} joined, {changes} lane changes, {gaps} gaps, {wrong} taken the wrong way round; {bad_length} lanes not as long as the file says");
    }
}

/// Consecutive route lanes that a vehicle can drive from one into the other: linked, a lane
/// change beside it, or starting (almost) where the first ends.
pub(super) fn joins(net: &Network, a: usize, b: usize) -> bool {
    net.lanes[a].next.contains(&b)
        || net.parallel(a, b)
        || (net.lanes[b].start() - net.lanes[a].end()).truncate().length() < 2.0
}

/// A station link often runs on past its station: the path search that made it went a
/// few paths beyond the stop - into a turning lane, round a corner - before the next link
/// starts back at the stop on another path (Spandau's links end so in 122 of 505 joins, the
/// extra paths mostly listed with length 0). Driven as listed, the bus turned off, then
/// jumped back and drove on the wrong side or against the traffic. Such a detour is passed
/// over (made `Absent`): where the route does not join, the lane a few steps back that
/// the next one continues from - or the lane a few steps on that continues this one - is
/// where the route really goes.
pub(super) fn skip_detours(net: &Network, slots: &mut [Slot]) {
    const REACH: usize = 8;
    let lane_at = |slots: &[Slot], k: usize| match slots[k] {
        Slot::Lane(l) => Some(l),
        _ => None,
    };
    let mut i = 0;
    while i + 1 < slots.len() {
        let (Some(a), Some(b)) = (lane_at(slots, i), lane_at(slots, i + 1)) else {
            i += 1;
            continue;
        };
        if joins(net, a, b) {
            i += 1;
            continue;
        }
        // back: an earlier lane of the route that `b` continues
        let back = (i.saturating_sub(REACH)..i)
            .rev()
            .find(|&k| lane_at(slots, k).map(|x| joins(net, x, b)).unwrap_or(false));
        // on: a later lane that continues `a`
        let on = (i + 2..(i + 2 + REACH).min(slots.len()))
            .find(|&k| lane_at(slots, k).map(|x| joins(net, a, x)).unwrap_or(false));
        match (back, on) {
            (Some(k), Some(m)) if i - k <= m - i - 1 => slots[k + 1..=i].fill(Slot::Absent),
            (_, Some(m)) => slots[i + 1..m].fill(Slot::Absent),
            (Some(k), None) => slots[k + 1..=i].fill(Slot::Absent),
            (None, None) => {}
        }
        i += 1;
    }
}

/// Where consecutive lanes of a route do not join (a path the timetable file names that
/// the map does not have any more, a junction a mod map edited after its tracks were
/// made), the shortest way between them through the network, when there is one not much
/// longer than the gap: the bus drives it instead of jumping across. Returns the lanes and,
/// for each lane given, its index in them.
pub(super) fn bridge_gaps(net: &Network, lanes: &[usize]) -> (Vec<usize>, Vec<usize>) {
    let mut out: Vec<usize> = Vec::with_capacity(lanes.len());
    let mut index = Vec::with_capacity(lanes.len());
    for (k, &b) in lanes.iter().enumerate() {
        if k > 0 {
            let a = lanes[k - 1];
            if !joins(net, a, b) {
                let gap = (net.lanes[b].start() - net.lanes[a].end()).truncate().length();
                if let Some(way) = way_between(net, a, b, (gap * 2.5 + 60.0) as f32) {
                    out.extend(way);
                }
            }
        }
        index.push(out.len());
        out.push(b);
    }
    (out, index)
}

/// The lanes strictly between `a` and `b` on the shortest way from the end of `a` to the
/// start of `b`, if that is at most `max` metres long.
pub(super) fn way_between(net: &Network, a: usize, b: usize, max: f32) -> Option<Vec<usize>> {
    use std::cmp::Reverse;
    let mut best: HashMap<usize, (f32, usize)> = HashMap::new();
    let mut heap = std::collections::BinaryHeap::new();
    for &n in &net.lanes[a].next {
        heap.push((Reverse(ordered(0.0)), n, a));
    }
    while let Some((Reverse(c), l, from)) = heap.pop() {
        let c = c as f32 / 1000.0;
        if best.contains_key(&l) {
            continue;
        }
        best.insert(l, (c, from));
        if l == b {
            let mut way = Vec::new();
            let mut at = from;
            while at != a {
                way.push(at);
                at = best.get(&at)?.1;
            }
            way.reverse();
            return Some(way);
        }
        let c2 = c + net.lanes[l].length();
        if c2 > max {
            continue;
        }
        for &n in &net.lanes[l].next {
            if !best.contains_key(&n) {
                heap.push((Reverse(ordered(c2)), n, l));
            }
        }
    }
    None
}

/// A distance in millimetres, for ordering.
pub(super) fn ordered(m: f32) -> u64 {
    (m.max(0.0) * 1000.0) as u64
}

/// A flight path: aircraft are not tied to the ground under them.
pub(super) fn track_is_air(traffic: &Traffic, lane: usize) -> bool {
    traffic
        .net
        .lanes
        .get(lane)
        .map(|l| l.kind == omsi_sim::traffic::LaneKind::Air)
        .unwrap_or(false)
}

/// Where on its route a bus is: the step it is on and how far into it, from the leg it is on
/// (`leg`, `frac` of the way along) and the estimated length of every step (`est`; an absent
/// step has none, so the bus is on the next step there is). None when that is past the end.
pub(super) fn step_at(
    steps: &[Step],
    slots: &[Slot],
    est: &[f64],
    leg: usize,
    frac: f64,
) -> Option<(usize, f64)> {
    let in_leg: Vec<usize> = (0..steps.len()).filter(|&k| steps[k].leg == leg).collect();
    let (mut at, mut offset) = match in_leg.last() {
        // a leg without a station link: the bus is at the start of the next one
        None => (steps.iter().position(|s| s.leg > leg)?, 0.0),
        Some(&last) => {
            let mut target = frac * in_leg.iter().map(|&k| est[k]).sum::<f64>();
            let mut pick = (last, est[last]);
            for &k in &in_leg {
                if est[k] > 0.0 && target <= est[k] {
                    pick = (k, target);
                    break;
                }
                target -= est[k];
            }
            pick
        }
    };
    while slots.get(at) == Some(&Slot::Absent) {
        at += 1;
        offset = 0.0;
    }
    (at < slots.len()).then_some((at, offset))
}

/// The steps around `at` that the network has, up to the steps still to come on either
/// side: (first, end).
pub(super) fn section_around(slots: &[Slot], at: usize) -> (usize, usize) {
    let start = slots[..at]
        .iter()
        .rposition(|s| *s == Slot::Waiting)
        .map(|k| k + 1)
        .unwrap_or(0);
    let end = slots[at..]
        .iter()
        .position(|s| *s == Slot::Waiting)
        .map(|k| at + k)
        .unwrap_or(slots.len());
    (start, end)
}
