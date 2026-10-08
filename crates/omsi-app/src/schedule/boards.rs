//! The departure boards at the stops.

use super::*;

impl Schedule {
    /// The buses due at one bus stop (map object id) within the next two hours, unsorted, as
    /// (expected arrival, line, terminus, time it stands at the stop), all in seconds of the day:
    /// the timetable buses with the delay they run with, and the player's.
    pub(super) fn stop_list(
        &self,
        stop: i64,
        now: f64,
        on_road: &HashMap<usize, OnRoad>,
        duty: Option<&PlayerDuty>,
        player_hof: Option<&omsi_vehicle::Hof>,
    ) -> Vec<(f64, String, String, f64)> {
        let mut list: Vec<(f64, String, String, f64)> = Vec::new();
        for &(trip, k) in self.visits.get(&stop).map(|v| v.as_slice()).unwrap_or(&[]) {
            for &i in &self.trip_departures[trip] {
                if !self.runs(i) {
                    continue;
                }
                let d = &self.departures[i];
                let tt = &self.times[d.trip][d.profile];
                if !tt.stops[k] {
                    continue;
                }
                let (arrive, leave) = (d.time + tt.stations[k].0, d.time + tt.stations[k].1);
                if arrive > now + BOARD_AHEAD || leave < now - BOARD_AHEAD {
                    continue;
                }
                if self.is_player_tour(i) {
                    continue;
                }
                let expected = match on_road.get(&i) {
                    Some(r) => match r.next {
                        // the stations before the bus's next stop are behind it
                        Some(next) if leave < next - 0.5 => continue,
                        None => continue,
                        // standing at this stop
                        Some(next) if r.dwelling && (leave - next).abs() < 0.5 => now,
                        Some(next) => {
                            // on its way: at least as late as it left its last stop, and
                            // later still once its next stop is overdue
                            let next_arrive = tt
                                .stations
                                .iter()
                                .find(|s| (d.time + s.1 - next).abs() < 0.5)
                                .map(|s| d.time + s.0)
                                .unwrap_or(next);
                            let late = if r.dwelling {
                                r.late
                            } else {
                                r.late.max(now - next_arrive)
                            };
                            (arrive + late).max(now)
                        }
                    },
                    // not on the road (still to come, or where no tiles are loaded): on time
                    None if leave < now => continue,
                    None => arrive.max(now),
                };
                let terminus = &self.data.trips[d.trip].terminus;
                let hof = self
                    .depots
                    .get(&d.ai_group.to_ascii_lowercase())
                    .and_then(|v| v.iter().find_map(|x| x.2.as_deref()));
                list.push((expected, self.display_line(i), terminus_text(hof, terminus), (leave - arrive).max(0.0)));
            }
        }
        // the player's bus
        if let Some(duty) = duty {
            // a trip not begun leaves on time at the earliest (the bus waits at its
            // first stop), one under way arrives as early or late as it runs
            let delay = duty.delay(now);
            let lateness = if duty.left_late().is_some() || duty.at_stop() {
                delay
            } else {
                delay.max(0.0)
            };
            for (ti, trip) in duty.trips.iter().enumerate().skip(duty.trip_index) {
                if trip.departure > now + BOARD_AHEAD {
                    break;
                }
                for (k, s) in trip
                    .stops
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.object_id == stop && s.stops)
                {
                    let current = ti == duty.trip_index;
                    let expected = if current
                        && (k < duty.next_stop || duty.trip_done() && k + 1 < trip.stops.len())
                    {
                        continue;
                    } else if current && k == duty.next_stop && duty.at_stop() {
                        now
                    } else if current {
                        (s.arr + lateness).max(now)
                    } else {
                        (s.arr + delay.max(0.0)).max(now)
                    };
                    list.push((
                        expected,
                        trip.line.trim().to_string(),
                        terminus_text(player_hof, &trip.terminus),
                        (s.dep - s.arr).max(0.0),
                    ));
                }
            }
        }
        list
    }

    /// Make the departure boards of the stops whose displays are near
    /// (`World::timetable_boards`), and hand the scenery the time of day. The boards are
    /// made at most once a second: the buses due at each stop in the next two hours,
    /// soonest first - the timetable buses with the delay they run with, and the player's.
    pub fn update_boards(
        &mut self,
        world: &World,
        traffic: Option<&Traffic>,
        duty: Option<&PlayerDuty>,
        player_hof: Option<&omsi_vehicle::Hof>,
        clock: &omsi_sim::SimClock,
    ) {
        let now = clock.time;
        let mut boards = world.timetable_boards.lock();
        boards.clock = Some(clock.clone());
        if (now - self.boards_made).abs() < 1.0 || (boards.wanted.is_empty() && boards.wanted_names.is_empty()) {
            return;
        }
        self.boards_made = now;
        // the timetable buses on the road: departure -> where they are in their trip (a
        // train runs its track without stops of its own and is taken as on time)
        let mut on_road: HashMap<usize, OnRoad> = HashMap::new();
        if let Some(t) = traffic {
            for car in &t.cars {
                let Some(&i) = self.car_departure.get(&car.id) else {
                    continue;
                };
                if trip_stations(&self.data.trips[self.departures[i].trip]).is_empty() {
                    continue;
                }
                let next = car.bus.as_ref().and_then(|b| b.stops.front()).map(|s| s.depart).or_else(|| {
                    self.running.iter().find(|r| r.car == car.id).and_then(|r| {
                        r.stations
                            .iter()
                            .zip(&r.served)
                            .find(|(_, s)| !**s)
                            .map(|(st, _)| st.1)
                    })
                });
                on_road.insert(
                    i,
                    OnRoad {
                        next,
                        dwelling: car.at_stop(),
                        late: car.bus.as_ref().map(|b| b.delay).unwrap_or(0.0).max(0.0),
                    },
                );
            }
        }
        let wanted = boards.wanted.clone();
        let mut made = HashMap::new();
        for stop in wanted {
            let mut list = self.stop_list(stop, now, &on_road, duty, player_hof);
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
            list.truncate(8);
            if omsi_cfg::flags::OMSI_DEBUG_BOARDS.is_set() {
                log::info!(
                    "board of stop {stop} at {:.0} s: {:?}",
                    now,
                    list.iter()
                        .map(|(t, l, d, _)| format!("{l} {d} in {:.1} min", (t - now) / 60.0))
                        .collect::<Vec<_>>()
                );
            }
            made.insert(stop, list.into_iter().map(|(t, l, d, _)| (l, d, t)).collect());
        }
        boards.by_stop = made;
        // the departures the pages asked for by stop name (`omsi.getDepartures`): the next two
        // hours, at most 20, as (line, destination, timestamp)
        let mut departures = std::collections::HashMap::new();
        for key in boards.wanted_names.clone() {
            let mut ids: Vec<i64> = self
                .data
                .bus_stops
                .iter()
                .filter(|b| b.name.trim().eq_ignore_ascii_case(&key))
                .map(|b| b.object_id)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            let mut list: Vec<(f64, String, String)> = Vec::new();
            for id in ids {
                for (expected, line, terminus, dwell) in self.stop_list(id, now, &on_road, duty, player_hof) {
                    let leaves = expected + dwell;
                    if leaves <= now + BOARD_AHEAD {
                        list.push((leaves, line, terminus));
                    }
                }
            }
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
            list.truncate(MAX_PAGE_DEPARTURES);
            departures.insert(
                key,
                list.into_iter()
                    .map(|(t, l, d)| (l, d, omsi_sim::vehicle_api::timestamp(clock, t)))
                    .collect(),
            );
        }
        boards.departures = departures;
        boards.departures_gen = boards.departures_gen.wrapping_add(1);
    }

    /// The line a departure's displays show: its trip's own (" 5"), else the timetable
    /// line's name.
    /// The name of station `i` (object `id`) of `trip`, as the map's stop calls it.
    pub(super) fn station_name(&self, trip: &omsi_timetable::Trip, i: usize, id: i64) -> String {
        self.data
            .bus_stops
            .iter()
            .find(|b| b.object_id == id)
            .map(|b| b.name.clone())
            .filter(|n| !n.trim().is_empty())
            // (the trip file names its stations too: a stop whose object is not
            // among the map's known stops - Novi Sad's, on tiles not loaded yet -
            // had no name on the navigator, the HUD and in the log)
            .or_else(|| {
                trip.stations
                    .is_empty()
                    .then(|| {
                        trip.stations_legacy
                            .get(i)
                            .and_then(|r| r.get(2))
                            .map(|n| n.trim().to_string())
                    })
                    .flatten()
            })
            .unwrap_or_default()
    }

    /// The station names of trip `ti`, for picking its route in the depot file.
    pub(super) fn trip_stop_names(&self, ti: usize) -> Vec<String> {
        let trip = &self.data.trips[ti];
        trip_stations(trip)
            .iter()
            .enumerate()
            .map(|(i, id)| self.station_name(trip, i, *id))
            .collect()
    }

    pub(super) fn display_line(&self, i: usize) -> String {
        let d = &self.departures[i];
        let own = self.data.trips[d.trip].line.trim();
        if own.is_empty() {
            d.line.trim().to_string()
        } else {
            own.to_string()
        }
    }
}

/// Where a timetable bus on the road is in its trip, for the departure boards.
pub(super) struct OnRoad {
    /// Timetable departure time of its next stop (None: it has served its last).
    next: Option<f64>,
    /// Standing at that stop.
    dwelling: bool,
    /// How late it left its last stop (s).
    late: f64,
}
