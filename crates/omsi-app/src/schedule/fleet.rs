//! The vehicles of the timetable: which bus runs a departure, and the vehicle sets read and
//! uploaded ahead.

use super::*;
use omsi_render::{Renderer, Scene};

/// How far ahead (s) the timetable reads and uploads the vehicles of its next departures: a
/// layover bus stands at its first stop a quarter of an hour early.
pub(super) const FLEET_AHEAD: f64 = 25.0 * 60.0;

/// [`FLEET_AHEAD`], or `OMSI_FLEET_AHEAD` minutes (for tests).
pub(super) fn fleet_ahead() -> f64 {
    omsi_cfg::flags::OMSI_FLEET_AHEAD
        .parse::<f64>()
        .map(|m| m * 60.0)
        .unwrap_or(FLEET_AHEAD)
}

/// A vehicle set nobody has drawn for this long, and that no departure of the next
/// minutes wants, leaves the GPU.
pub(super) const FLEET_IDLE: std::time::Duration = std::time::Duration::from_secs(90);

impl Schedule {
    /// Upload the vehicles of the buses on the road at `day_time` and of the departures of the
    /// next minutes before the first frame, so that they spawn without a hitch; the rest of
    /// the fleet follows as its departures come near (see [`Schedule::tick`]). Uploading the
    /// whole fleet in every paint scheme up front took 109 sets and 640 MB on Ahlheim.
    ///
    /// Every type of the fleet is started once, which reads the files its scripts and
    /// displays need: done on the first bus of a type that comes along, those were frames of
    /// 60 to 170 ms in the middle of a drive.
    pub fn precache(
        &mut self,
        world: &World,
        renderer: &Renderer,
        scene: &mut Scene,
        traffic: Option<&mut crate::traffic::Traffic>,
        day_time: f64,
    ) {
        let t0 = std::time::Instant::now();
        let Some(t) = traffic else { return };
        let sets = self.upcoming_sets(world, t, day_time);
        // a few sets at a time: read on the workers, uploaded, and the copies let go
        let mut most_held = 0usize;
        for chunk in sets.chunks(3) {
            most_held = most_held.max(world.prefetch_vehicle_sets(renderer, chunk));
            for (ty, scheme) in chunk {
                world.precache_vehicle(renderer, scene, ty, *scheme);
            }
        }
        let t1 = std::time::Instant::now();
        let mut seen = std::collections::HashSet::new();
        for (ty, _, hof) in self.depots.values().flatten() {
            if !seen.insert(ty.def.path.clone()) {
                continue;
            }
            crate::traffic::warm_up(world, ty, hof.clone());
            t.prime_pull_out_room(ty, true);
            for (tr, _) in t.trailer_chain(ty) {
                if seen.insert(tr.def.path.clone()) {
                    crate::traffic::warm_up(world, &tr, None);
                }
            }
        }
        let pooled: Vec<Arc<VehicleType>> = self.pools.values().flatten().cloned().collect();
        for ty in &pooled {
            if seen.insert(ty.def.path.clone()) {
                let hof = self.pool_hof(ty.def.dir(), world);
                crate::traffic::warm_up(world, ty, hof);
            }
        }
        // what was read ahead and not used (textures of variants the AI never shows)
        world.forget_prefetched();
        crate::release_free_memory();
        self.fleet_check = day_time;
        log::info!("timetable fleet: {} vehicle/paint sets of the first {:.0} minutes read and uploaded in {:.1} s (at most {:.0} MB read ahead at once), a first start of every type in {:.1} s", sets.len(), fleet_ahead() / 60.0, (t1 - t0).as_secs_f32(), most_held as f64 / 1e6, t1.elapsed().as_secs_f32());
    }

    /// The vehicle departure `i` is driven with (see [`Choice`]).
    pub(super) fn choose(&mut self, i: usize, world: &World) -> Option<Choice> {
        let group = self.departures[i].ai_group.to_ascii_lowercase();
        let h = self.tour_key(i);
        // trains: the group lists .zug files instead of depot vehicles
        let train = self
            .trains
            .get(&group)
            .and_then(|t| t.get((h % t.len().max(1) as u64) as usize).cloned());
        let (ty, numbers, hof): (
            Arc<VehicleType>,
            Vec<omsi_map::DepotEntry>,
            Option<Arc<omsi_vehicle::Hof>>,
        ) = match (&train, self.depots.get(&group).filter(|v| !v.is_empty())) {
            (Some(cars), _) => (cars[0].0.clone(), Vec::new(), None),
            (None, Some(vehicles)) => {
                // The depot's types come out in proportion to their fleets: a typgroup
                // listing 40 fleet numbers appears eight times as often as one with
                // 5, as in OMSI - a plain round robin gave the single MB O305 of a
                // depot the same share as the whole SD200 fleet.
                // A tour's vehicle for the day: the one `car_use` gives it, else one drawn now
                // and kept (a fleet number no other tour has, while there are any left).
                let (k, j) = match self.tour_vehicle.get(&h) {
                    Some(&kj) => kj,
                    None => {
                        let weights: Vec<usize> = vehicles.iter().map(|(_, n, _)| n.len().max(1)).collect();
                        let total: usize = weights.iter().sum();
                        let mut x = (h % total.max(1) as u64) as usize;
                        let mut k = 0usize;
                        for (i, w) in weights.iter().enumerate() {
                            if x < *w {
                                k = i;
                                break;
                            }
                            x -= w;
                        }
                        let nums = &vehicles[k].1;
                        let start = ((h >> 21) % nums.len().max(1) as u64) as usize;
                        let free = (0..nums.len())
                            .map(|o| (start + o) % nums.len())
                            .find(|&j| !self.used_numbers.contains(&(group.clone(), nums[j].number.trim().to_string())));
                        let j = free.unwrap_or(start);
                        if let Some(n) = nums.get(j) {
                            self.used_numbers.insert((group.clone(), n.number.trim().to_string()));
                        }
                        self.tour_vehicle.insert(h, (k, j));
                        (k, j)
                    }
                };
                let (ty, numbers, hof) = &vehicles[k.min(vehicles.len() - 1)];
                let numbers = numbers.get(j).cloned().into_iter().collect::<Vec<_>>();
                (ty.clone(), numbers, hof.clone())
            }
            _ => {
                // a plain [aigroup_2] flies/drives its own vehicles (the Tegel approach)
                let root = world.root.clone();
                let ty = {
                    let pool = self.pool(&root, world, &group);
                    pool.get((h % pool.len().max(1) as u64) as usize).cloned()?
                };
                // The group names no depot file, so Omsi.exe leaves the bus's selected-hof
                // index at 0: it runs with the depot of its own folder (see [`pool_depot`]).
                // Without one such a bus got no `SetLineTo`, no `AI_target_index` and no
                // `ai_scheduled_settarget` trigger at all - and a mod bus that switches its
                // destination picture on in that trigger drove with a blank display.
                let hof = self.pool_hof(ty.def.dir(), world);
                (ty, Vec::new(), hof)
            }
        };
        // A depot bus as Omsi.exe makes it (0x70a174): the fleet number of its ailists line;
        // the plate of that line, else - unless the bus's plates are free - the plate the bus
        // gives the number ([registration_list] / [registration_automatic]); and the repaint
        // that line names, else the model's own paint (the first repaint when the default
        // paint is "<nouse>"). Another tour's bus draws a repaint at random, as random
        // traffic does.
        let entry = numbers.first().cloned();
        let number = entry.as_ref().map(|e| {
            let plate = if !e.registration.trim().is_empty() {
                e.registration.clone()
            } else if ty.def.registration_mode != 1 {
                ty.def.plate_of_number(&e.number)
            } else {
                String::new()
            };
            (e.number.clone(), plate)
        });
        let scheme = if ty.paint_schemes.is_empty() {
            None
        } else if let Some(e) = &entry {
            ty.paint_schemes
                .iter()
                .position(|s| s.name.trim_end() == e.paint.trim_end())
                .or_else(|| (ty.def.default_paint.trim() == "<nouse>").then_some(0))
        } else {
            Some(
                ((h >> 42) % ty.paint_schemes.len().min(crate::traffic::AI_SCHEMES) as u64)
                    as usize,
            )
        };
        Some(Choice {
            ty,
            number,
            hof,
            scheme,
            train,
        })
    }

    /// The vehicle sets a choice is drawn with: the vehicle, its rear sections, a train's
    /// further cars.
    pub(super) fn choice_sets(c: &Choice, traffic: &mut Traffic) -> Vec<(Arc<VehicleType>, Option<usize>)> {
        let mut out = vec![(c.ty.clone(), c.scheme)];
        for (t, _) in traffic.trailer_chain(&c.ty) {
            let s = c.scheme.filter(|i| *i < t.paint_schemes.len());
            out.push((t, s));
        }
        if let Some(cars) = &c.train {
            out.extend(cars.iter().skip(1).map(|(t, _)| (t.clone(), None)));
        }
        out
    }

    /// The vehicle sets of the trips on the road at `day_time` and of the departures of the
    /// next [`FLEET_AHEAD`] seconds.
    pub(super) fn upcoming_sets(
        &mut self,
        world: &World,
        traffic: &mut Traffic,
        day_time: f64,
    ) -> Vec<(Arc<VehicleType>, Option<usize>)> {
        let mut out = Vec::new();
        let mut seen: HashSet<crate::scene::VehicleKey> = HashSet::new();
        let end = self
            .departures
            .partition_point(|d| d.time <= day_time - self.day_base + fleet_ahead());
        for i in 0..end {
            if self.is_player_tour(i) || !self.runs(i) {
                continue;
            }
            if self.dep_time(i) + self.times_of(i).duration < day_time {
                continue;
            }
            let Some(c) = self.choose(i, world) else {
                continue;
            };
            for (ty, scheme) in Self::choice_sets(&c, traffic) {
                if seen.insert((ty.def.path.clone(), scheme)) {
                    out.push((ty, scheme));
                }
            }
        }
        out
    }

    /// Keep the GPU's fleet to the vehicles of the next minutes: read the sets of the
    /// coming departures on the workers and upload them (one a frame) well before they are
    /// due, and let go of the sets nobody uses any more.
    pub(super) fn fleet(
        &mut self,
        world: &World,
        traffic: &mut Traffic,
        renderer: &Renderer,
        scene: &mut Scene,
        day_time: f64,
    ) {
        let ready = self.fleet_ready.lock().pop();
        if let Some(key) = ready {
            if let Some(ty) = self.fleet_reading.remove(&key) {
                let t = std::time::Instant::now();
                world.precache_vehicle(renderer, scene, &ty, key.1);
                if omsi_cfg::flags::OMSI_PROFILE.is_set() {
                    log::info!(
                        "timetable fleet: {} (scheme {:?}) uploaded ahead in {:.1} ms",
                        ty.def
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy(),
                        key.1,
                        t.elapsed().as_secs_f64() * 1000.0
                    );
                }
            }
        }
        if (day_time - self.fleet_check).abs() < 5.0 {
            return;
        }
        self.fleet_check = day_time;
        let sets = self.upcoming_sets(world, traffic, day_time);
        let keep: HashSet<crate::scene::VehicleKey> = sets
            .iter()
            .map(|(t, s)| (t.def.path.clone(), *s))
            .chain(self.fleet_reading.keys().cloned())
            .chain(traffic.random_sets().into_iter().map(|(t, s)| (t.def.path.clone(), s)))
            .collect();
        // (OMSI_FLEET_IDLE=<s> shortens the wait, for tests)
        let idle = omsi_cfg::flags::OMSI_FLEET_IDLE
            .parse::<f32>()
            .map(std::time::Duration::from_secs_f32)
            .unwrap_or(FLEET_IDLE);
        if world.trim_vehicle_sets(renderer, scene, &keep, idle) > 0 {
            crate::release_free_memory();
        }
        let prefetch = world.vehicle_prefetch(renderer);
        for (ty, scheme) in sets {
            let key = (ty.def.path.clone(), scheme);
            if self.fleet_reading.contains_key(&key) || world.has_vehicle_set(&key) {
                continue;
            }
            // two at a time: a set's repaints are compressed from pictures of tens of
            // megabytes (the rest follows at the next look, five seconds on)
            if self.fleet_reading.len() >= 2 {
                break;
            }
            self.fleet_reading.insert(key.clone(), ty.clone());
            let (p, ready) = (prefetch.clone(), self.fleet_ready.clone());
            // (off the frame's pool: see `threads`)
            crate::threads::background_pool().spawn(move || {
                p.prefetch(&ty, scheme);
                ready.lock().push(key);
            });
        }
    }

    /// Vehicles of a plain `[aigroup_2]`, loaded on first use and kept.
    pub(super) fn pool(&mut self, root: &Path, world: &World, group: &str) -> &[Arc<VehicleType>] {
        if !self.pools.contains_key(group) {
            let mut out = Vec::new();
            for g in world
                .ailists
                .groups
                .iter()
                .filter(|g| !g.is_depot && g.name.to_ascii_lowercase() == group)
            {
                for v in &g.vehicles {
                    if v.file.to_ascii_lowercase().ends_with(".zug") {
                        continue;
                    }
                    let path = omsi_cfg::resolve_path(root, &v.file);
                    match VehicleType::load_ai(root, &path) {
                        Ok(t) => out.push(Arc::new(t)),
                        Err(e) => log::warn!("AI group '{}' vehicle {}: {e}", g.name, v.file),
                    }
                }
            }
            if !out.is_empty() {
                log::info!(
                    "AI group '{group}': {} vehicles loaded for its timetable trips",
                    out.len()
                );
            }
            self.pools.insert(group.to_string(), out);
        }
        self.pools.get(group).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// The depot file a bus of a plain `[aigroup_2]` group runs with (see [`pool_depot`]),
    /// looked up once per vehicle folder.
    pub(super) fn pool_hof(&mut self, dir: &Path, world: &World) -> Option<Arc<omsi_vehicle::Hof>> {
        if let Some(h) = self.pool_hofs.get(dir) {
            return h.clone();
        }
        let names: Vec<&str> = world
            .ailists
            .groups
            .iter()
            .filter_map(|g| g.hof.as_deref())
            .collect();
        let h = pool_depot(dir, &names).map(Arc::new);
        self.pool_hofs.insert(dir.to_path_buf(), h.clone());
        h
    }
}

/// The depot file of a bus whose AI group names none (a plain `[aigroup_2]` pool): the one
/// of the map's own depots where its vehicle folder has it (the group is the map author's
/// shortcut for a fleet the `[aigroup_depot]` groups name with a depot), else the folder's
/// first `.hof` - what Omsi.exe uses for such a bus, whose selected-hof index stays 0
/// ("the hofs are loaded in the order the folder lists them", [`omsi_vehicle::hof::depot_files`]).
///
/// A timetable bus is spawned with this depot file: with none it got no `SetLineTo`, no
/// `AI_target_index` and no `ai_scheduled_settarget` trigger, and a mod bus that switches
/// its destination picture on in that trigger (the HK roller blinds, the LED matrices)
/// drove with a blank display instead of its destination.
pub(super) fn pool_depot(dir: &Path, map_depots: &[&str]) -> Option<omsi_vehicle::Hof> {
    omsi_vehicle::hof::depot_like(dir, map_depots).or_else(|| {
        let files = omsi_vehicle::hof::depot_files(dir);
        // (the first file that loads: one unreadable depot must not leave the bus without
        // the displays and the stops of the rest)
        files.iter().find_map(|p| omsi_vehicle::Hof::load(p).ok())
    })
}

/// The depot file an `[aigroup_depot]` names for a vehicle: a file of that name next to the
/// vehicle, else the one whose `[name]` it is - the stock groups name the depot
/// ("Spandau 1986"), not the file ("Spandau 86.hof"), and without it no scheduled bus had
/// termini or stops for its displays.
pub(super) fn depot_file(
    cache: &mut HashMap<(std::path::PathBuf, String), Option<Arc<omsi_vehicle::Hof>>>,
    dir: &Path,
    name: &str,
) -> Option<Arc<omsi_vehicle::Hof>> {
    let key = (dir.to_path_buf(), name.trim().to_ascii_lowercase());
    if let Some(h) = cache.get(&key) {
        return h.clone();
    }
    // (through the content file system: the bus may be in an archive, and a mod may add
    // depot files to a stock bus folder)
    let mut found = omsi_vehicle::hof::depot_in(dir, name);
    if found.is_none() {
        // a mod bus brings only the depot of the map it was made on: the map's depot as
        // another vehicle folder has it (`omsi_vehicle::hof::depot_anywhere`)
        found = omsi_vehicle::hof::depot_anywhere(name);
        match &found {
            Some(h) => log::info!(
                "{} has no depot file '{name}'; using {}",
                dir.display(),
                h.path.display()
            ),
            None => log::debug!(
                "no depot file '{name}' next to {} or in any vehicle folder",
                dir.display()
            ),
        }
    }
    let h = found.map(Arc::new);
    cache.insert(key, h.clone());
    h
}
