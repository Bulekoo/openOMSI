//! Making the traffic: the network from the tiles, the AI vehicle types of the map.

use super::*;
use crate::scene::World;

/// Start a vehicle of type `ty` once and throw it away: its `{init}` and its displays read
/// the files they need (depot data, fonts) into the caches before the first real one of the
/// type comes along in the middle of a drive.
pub fn warm_up(world: &World, ty: &Arc<VehicleType>, hof: Option<Arc<omsi_vehicle::Hof>>) {
    let t = std::time::Instant::now();
    let mut host = omsi_sim::VehicleHost::new(omsi_sim::SimClock::default());
    host.hof = hof;
    host.font_lib = Some(world.fonts.clone());
    let mut vehicle = VehicleInstance::new(ty.clone(), host);
    vehicle.init_text_textures(&mut world.fonts.lock(), &|p| {
        omsi_texture::decode_file(p)
            .ok()
            .map(|i| (i.width, i.height, i.rgba))
    });
    if omsi_cfg::env::var_os("OMSI_PROFILE").is_some() {
        log::info!(
            "  first start of {}: {:.1} ms",
            ty.def.path.display(),
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}

/// The `OMSI_TRACE_AI` file, with its header written.
pub(super) fn open_trace() -> Option<std::io::BufWriter<std::fs::File>> {
    use std::io::Write;
    let path = omsi_cfg::env::var_os("OMSI_TRACE_AI")?;
    let mut f = std::io::BufWriter::new(
        std::fs::File::create(&path)
            .map_err(|e| log::warn!("OMSI_TRACE_AI: {e}"))
            .ok()?,
    );
    writeln!(f, "t,id,type,x,y,z,heading,pitch,bank,steer,speed,lane,s,blinker,turn,lane_heading,lateral,at_station,acc,yielding,light_hold,passing,front,rear,half_width,scheduled,why,why_gap,phase,lane_z").ok()?;
    Some(f)
}

impl Traffic {
    /// Make the population deterministic for a LAN room.  The room's session id is
    /// shared by the host and every client, so the same map/time produces the same
    /// initial cars instead of each process inventing a different world.
    pub fn set_lan_seed(&mut self, seed: u64) {
        self.rng = seed | 1;
    }

    /// Build the network from the lanes collected by `World::build_scene` and load the AI
    /// car types of the map's `ailists.cfg` (the `[aigroup_2]` groups that are not depots).
    pub fn new(root: &Path, world: &World, target: usize) -> Result<Traffic> {
        let (lanes, parked_cars, lane_tiles) = take_from_tiles(world);
        let mut net = Network {
            lanes,
            // (`[lht]`: priority to the left, passing on the right, keeping left)
            left_hand: world.global.left_hand_traffic,
            ..Default::default()
        };
        if net.left_hand {
            log::info!("traffic: the map drives on the left");
        }
        net.link(1.5);
        let RandomTypes { types, groups, group_curves, group_uvg, uvg_defaults } = random_types(root, world);
        // No buses in the random road traffic: the depot groups of `ailists.cfg` are the
        // fleet the *timetable* drives, and OMSI puts a bus on a street only because a trip
        // of the map's TTData runs there. Mixing the depot fleet into the random pool put
        // the map's one bus type on every road of the map - on Grundorf that is a single
        // articulated GN92, which is why it seemed to be a type of our own choosing.
        log_debug_lanes(&net);
        if omsi_cfg::env::var_os("OMSI_DEBUG_WHEELS").is_some() {
            for (t, ..) in &types {
                let v = VehicleInstance::new(
                    t.clone(),
                    omsi_sim::VehicleHost::new(omsi_sim::SimClock::default()),
                );
                for line in v.wheel_pivot_report() {
                    log::info!("wheel pivot: {line}");
                }
            }
        }
        let lights = world.traffic_lights.lock().clone();
        let controller_of_object = world.controller_of_object.lock().clone();
        let turning = net.lanes.iter().filter(|l| l.turn != 0).count();
        let with_side = net
            .lanes
            .iter()
            .filter(|l| l.left.is_some() || l.right.is_some())
            .count();
        let turn_lanes = net
            .lanes
            .iter()
            .filter(|l| {
                (l.left.is_some() || l.right.is_some())
                    && l.next.iter().any(|&n| net.lanes[n].turn != 0)
            })
            .count();
        let closed = net
            .lanes
            .iter()
            .filter(|l| l.no_cars || l.density <= 0.0)
            .count();
        let closed_junctions = net
            .lanes
            .iter()
            .filter(|l| (l.no_cars || l.density <= 0.0) && l.source == 2)
            .count();
        let quiet = net
            .lanes
            .iter()
            .filter(|l| l.density < 1.0 && l.density > 0.0)
            .count();
        let prio = net
            .lanes
            .iter()
            .filter(|l| (l.priority - omsi_sim::traffic::DEFAULT_PRIORITY).abs() > 0.5)
            .count();
        log::info!("traffic: {} lanes ({turning} turning, {with_side} with a neighbour, {turn_lanes} where a turn lane applies, {closed} closed to cars of which {closed_junctions} are junctions, {quiet} with less traffic by [rule], {prio} with a [rule] priority), {} AI vehicle types in {} groups, {} light programs, {} lamps", net.lanes.len(), types.len(), groups.len(), lights.len(), world.light_objects.lock().len());
        // lights on paths a car reaches from a lit path of the same crossing (they hold
        // only a car that comes into the crossing there, see `light_at_entry`)
        let inner_lights = (0..net.lanes.len())
            .filter(|&l| {
                let lane = &net.lanes[l];
                lane.traffic_light.is_some()
                    && lane.source == 2
                    && net.prev.get(l).is_some_and(|ps| {
                        ps.iter().any(|&p| {
                            let q = &net.lanes[p];
                            q.source == 2 && q.traffic_light.is_some() && q.key.map(|k| (k.tile, k.id)) == lane.key.map(|k| (k.tile, k.id))
                        })
                    })
            })
            .count();
        log::info!("traffic: {inner_lights} lit paths inside crossings (a car already in the crossing is not held there again)");
        let density_curve = world.global.traffic_density_road.clone();
        let unsched_factor = crate::settings::Settings::load().ai_unsched_factor;
        let max_scheduled = crate::settings::Settings::load().ai_max_scheduled;
        let random = RandomTypes { types, groups, group_curves, group_uvg, uvg_defaults };
        Ok(Traffic::assemble(root, net, random, lights, controller_of_object, (parked_cars, lane_tiles), density_curve, (unsched_factor, max_scheduled), target))
    }

    /// The traffic made of its parts: the linked network, the random traffic's types,
    /// the light programs (and which crossing object has which), the parked cars and
    /// tiles the network came from, the map's density curve and the options' share of
    /// random traffic and number of timetable vehicles. Nothing in it needs a world or a
    /// GPU (see `Traffic::new`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn assemble(
        root: &Path,
        net: Network,
        random: RandomTypes,
        lights: Vec<TrafficLightController>,
        controller_of_object: HashMap<i64, usize>,
        (parked_cars, lane_tiles): (Vec<(DVec3, f64)>, Tiles),
        density_curve: Vec<(f32, f32)>,
        (unsched_factor, max_scheduled): (f32, u32),
        target: usize,
    ) -> Traffic {
        let RandomTypes { types, groups, group_curves, group_uvg, uvg_defaults } = random;
        let parked: HashMap<usize, Vec<(f32, f32)>> = HashMap::new();
        let light_log = omsi_cfg::env::var("OMSI_DEBUG_LIGHTS").ok();
        let light_prev = lights.iter().map(|c| vec![-100; c.lights.len()]).collect();
        let lanes = 0..net.lanes.len();
        let street_weight = net.lanes.iter().filter_map(street_lane_weight).sum();
        let mut t = Traffic {
            net,
            street_weight,
            parked,
            parked_waiting: Vec::new(),
            lane_tiles: lane_tiles.into_iter().collect(),
            lanes_generation: 0,
            types,
            groups,
            group_curves,
            group_uvg,
            uvg_defaults: Arc::new(uvg_defaults),
            cars: Vec::new(),
            dormant: Vec::new(),
            dormant_time: 0.0,
            rng: 0x9E37_79B9_7F4A_7C15,
            target,
            lights_only: false,
            spawn_radius: 400.0,
            time: 0.0,
            view: TrafficView::default(),
            camera: None,
            orphan_sounds: Vec::new(),
            lights,
            controller_of_object,
            trailer_types: HashMap::new(),
            sound_cfgs: HashMap::new(),
            root: root.to_path_buf(),
            held_at_red: 0,
            stop_wishes: None,
            player_still: 0.0,
            day_time: 0.0,
            time_scale: 1.0,
            weekday: 0,
            night: false,
            daylight: None,
            next_id: 1,
            last_overtaker: None,
            first_turner: None,
            first_red: None,
            first_yield: None,
            first_passer: None,
            density_curve,
            unsched_factor,
            max_scheduled,
            no_timetable_buses: false,
            viewer: None,
            occluders: None,
            walkers: Vec::new(),
            people: Vec::new(),
            initial: true,
            last_dt: 0.0,
            lamp_dt: 0.0,
            trace: open_trace(),
            logged_hard: Default::default(),
            light_log,
            light_prev,
            debug_population: omsi_cfg::env::var_os("OMSI_DEBUG_POPULATION").is_some(),
            framed_spawns: Vec::new(),
            player: None,
            player_priority: false,
            player_blinker: 0,
            player_signal_age: f32::MAX,
            player_signalling: 0.0,
            way_users: Vec::new(),
            others: Vec::new(),
            tick_split: [0.0; 3],
            others_still: HashMap::new(),
            geo_prev: Vec::new(),
            index_of: HashMap::new(),
            pull_out_rooms: HashMap::new(),
            removed_scheduled: Vec::new(),
            twinned: Default::default(),
            keep_clear: Vec::new(),
            mirror: false,
            count_near: None,
            lan_centers: Vec::new(),
        };
        t.sort_parked(parked_cars, lanes);
        t
    }

    /// Take in what the tiles loaded since the last call brought: their lanes (linked into
    /// the network, whose existing indices stay valid), their parked cars (sorted onto the
    /// lanes once those are in) and the light programs of their crossings.
    pub fn add_tiles(&mut self, world: &World) -> usize {
        let (new, parked_cars, tiles) = take_from_tiles(world);
        let n = new.len();
        let mut added = self.net.lanes.len()..self.net.lanes.len();
        if n > 0 {
            added = self.net.extend(new, 1.5);
            self.street_weight += self.net.lanes[added.clone()].iter().filter_map(street_lane_weight).sum::<f64>();
            log::debug!(
                "traffic: {} lanes added ({} in all)",
                added.len(),
                self.net.lanes.len()
            );
        }
        if !tiles.is_empty() {
            self.lane_tiles.extend(tiles);
            self.lanes_generation += 1;
        }
        self.sort_parked(parked_cars, added);
        // new crossings bring their light programs; the running ones keep their clocks
        // (a program is never taken away again: the world's list only grows)
        let lights = world.traffic_lights.lock();
        if lights.len() > self.lights.len() {
            let from = self.lights.len();
            self.lights.extend(lights[from..].iter().cloned());
            self.light_prev
                .extend(lights[from..].iter().map(|c| vec![-100; c.lights.len()]));
            self.controller_of_object = world.controller_of_object.lock().clone();
        }
        n
    }
}

/// What the tiles placed since the last call hand to the traffic: their lanes, their parked
/// cars and which tiles they were (taken together, see `World::lane_tiles`).
pub(super) fn take_from_tiles(
    world: &World,
) -> (
    Vec<omsi_sim::traffic::Lane>,
    Vec<(DVec3, f64)>,
    Vec<(i32, i32)>,
) {
    let mut lanes = world.lanes.lock();
    let parked = std::mem::take(&mut *world.parked_cars.lock());
    let tiles = std::mem::take(&mut *world.lane_tiles.lock());
    let mut new = std::mem::take(&mut *lanes);
    // The tiles are read in parallel and hand in their lanes in the order they finish:
    // sorted by their map identity, the lanes a set of tiles brings are numbered alike in
    // every run, and so is the random traffic drawn from them (a run can be repeated to
    // look at what a car did).
    new.sort_by(|a, b| {
        let first = |l: &omsi_sim::traffic::Lane| {
            l.points
                .first()
                .map(|p| (p.x.to_bits(), p.y.to_bits()))
                .unwrap_or((0, 0))
        };
        (
            a.key.map(|k| (k.tile, k.id, k.path)),
            a.reversed,
            a.source,
            first(a),
        )
            .cmp(&(
                b.key.map(|k| (k.tile, k.id, k.path)),
                b.reversed,
                b.source,
                first(b),
            ))
    });
    (new, parked, tiles)
}

/// Tiles by their grid position.
pub(super) type Tiles = Vec<(i32, i32)>;

/// The random traffic's vehicle types (with weight, the lanes they run on and their group),
/// its groups with their density curves and the paths' density rules for them.
pub(super) struct RandomTypes {
    pub(super) types: Vec<(Arc<VehicleType>, f32, LaneKind, usize)>,
    pub(super) groups: Vec<omsi_map::ailists::UnschedGroup>,
    pub(super) group_curves: bool,
    pub(super) group_uvg: Vec<Option<usize>>,
    pub(super) uvg_defaults: Vec<i32>,
}

/// Load the AI car types of the map's `ailists.cfg` that make its random traffic.
fn random_types(root: &Path, world: &World) -> RandomTypes {
    let mut types = Vec::new();
    let mut groups: Vec<omsi_map::ailists::UnschedGroup> = Vec::new();
    let mut group_uvg: Vec<Option<usize>> = Vec::new();
    let mut uvg_defaults: Vec<i32> = Vec::new();
    // `unsched_trafficdens.txt`: per random group a factor and its density over the day
    // (by day of the week); the global.cfg curve is the fallback of maps without it
    let dens: Vec<omsi_map::ailists::UnschedGroup> =
        omsi_cfg::CfgFile::read(&world.map_dir.join("unsched_trafficdens.txt"))
            .ok()
            .map(|f| omsi_map::ailists::parse_unsched_trafficdens(&f))
            .unwrap_or_default();
    let group_curves = !dens.is_empty();
    {
        // `unsched_vehgroups.txt` names the groups the random traffic is made of. The
        // other `[aigroup_2]`s exist only for the timetable: on Berlin-Spandau "Pan Am"
        // and "Mi-8 Soviet AF" fly TXL.ttl and Relais.ttl, and taking them into the
        // random pool put airliners on the flight paths at any hour of the day. Its
        // number is the group's default density on the paths without a `[rule]
        // trafficdensity` for it (see `uvg_density`): 0 means only where the paths ask
        // for the group. Taken as "off", Spandau had no trucks and no Trabant at all,
        // though 865 paths ask for the one and 462 around Falkensee for the other.
        // `OMSI_TRAFFIC_ALL_GROUPS=1` lets such groups drive everywhere (and, on a map
        // without the file, every group, not only the default one).
        let all_groups = omsi_cfg::env::var_os("OMSI_TRAFFIC_ALL_GROUPS").is_some();
        let unscheduled: Option<Vec<(String, i32)>> =
            omsi_cfg::CfgFile::read(&world.map_dir.join("unsched_vehgroups.txt"))
                .ok()
                .map(|f| {
                    omsi_map::ailists::parse_unsched_vehgroups(&f)
                        .into_iter()
                        .map(|(n, c)| (n.trim().to_ascii_lowercase(), c))
                        .collect()
                });
        if let Some(names) = &unscheduled {
            log::info!("random traffic groups (unsched_vehgroups.txt): {names:?}");
            uvg_defaults = names
                .iter()
                .map(|n| if all_groups && n.1 <= 0 { 1 } else { n.1 })
                .collect();
        }
        let lists = &world.ailists;
        // Without `unsched_vehgroups.txt` the random traffic is the ailists' default group
        // alone (the first, or the one the `[ailist]` header names): Omsi.exe 0x785f98
        // makes one nameless group then, and a nameless group takes the default group.
        // Taking every group instead, a map whose ailists keep an ambulance (or a bus,
        // or a lorry) in a group of its own had one car in four of that kind (#1025).
        if unscheduled.is_none() && !all_groups {
            if let Some(g) = lists.groups.get(lists.default_group) {
                log::info!("random traffic: no unsched_vehgroups.txt, only the default AI group {}", g.name);
            }
        }
        for (_, g) in lists.groups.iter().enumerate().filter(|(i, g)| {
            !g.is_depot
                && g.hof.is_none()
                && (unscheduled.is_some() || all_groups || *i == lists.default_group)
                && !g
                    .vehicles
                    .iter()
                    .any(|v| v.file.to_ascii_lowercase().ends_with(".zug"))
        }) {
            let lname = g.name.trim().to_ascii_lowercase();
            let uvg = match &unscheduled {
                Some(names) => match names.iter().position(|n| n.0 == lname) {
                    None => continue,
                    Some(u) => {
                        if uvg_defaults.get(u).copied().unwrap_or(0) <= 0 {
                            log::info!(
                                "random traffic group {} drives only where its paths ask for it (unsched_vehgroups.txt)",
                                g.name
                            );
                        }
                        Some(u)
                    }
                },
                None => None,
            };
            let gi = groups.len();
            group_uvg.push(uvg);
            groups.push(
                dens.iter()
                    .find(|d| d.name.trim().eq_ignore_ascii_case(g.name.trim()))
                    .cloned()
                    .unwrap_or(omsi_map::ailists::UnschedGroup {
                        name: g.name.clone(),
                        factor: if group_curves { 0.0 } else { 1.0 },
                        densities: Vec::new(),
                    }),
            );
            for v in &g.vehicles {
                let lower = v.file.to_ascii_lowercase();
                if lower.ends_with(".zug")
                    || lower.contains("trains\\")
                    || lower.contains("trains/")
                {
                    continue;
                }
                let path = omsi_cfg::resolve_path(root, &v.file);
                match VehicleType::load_ai(root, &path) {
                    Ok(t) => {
                        // rail (only as scheduled trains), 3 = aircraft on flight paths
                        let rail = t.def.is_rail();
                        let air =
                            matches!(t.def.kind, omsi_vehicle::vehicle::VehicleKind::Other(3));
                        if rail {
                            log::debug!(
                                "AI vehicle {} is rail-bound, not street traffic",
                                v.file
                            );
                        } else {
                            types.push((
                                Arc::new(t),
                                v.weight.max(0.0),
                                if air { LaneKind::Air } else { LaneKind::Street },
                                gi,
                            ));
                        }
                    }
                    Err(e) => log::warn!("AI vehicle {}: {e}", v.file),
                }
            }
        }
    }
    RandomTypes { types, groups, group_curves, group_uvg, uvg_defaults }
}

/// `OMSI_DEBUG_LANES`: the chosen lanes as the network has them.
fn log_debug_lanes(net: &Network) {
    if let Ok(list) = omsi_cfg::env::var("OMSI_DEBUG_LANES") {
        // lane indices, or `at:x,y,r` for the street lanes passing within r m of a point
        // (the indices change from run to run on a map whose tiles load in parallel)
        let chosen: Vec<usize> = match list.strip_prefix("at:") {
            Some(rest) => {
                let v: Vec<f64> = rest
                    .split(',')
                    .filter_map(|x| x.trim().parse().ok())
                    .collect();
                let (p, r) = (
                    DVec3::new(
                        v.first().copied().unwrap_or(0.0),
                        v.get(1).copied().unwrap_or(0.0),
                        0.0,
                    ),
                    v.get(2).copied().unwrap_or(10.0),
                );
                (0..net.lanes.len())
                    .filter(|&i| {
                        net.lanes[i].kind == LaneKind::Street
                            && net.lanes[i]
                                .points
                                .iter()
                                .any(|q| (q.truncate() - p.truncate()).length() < r)
                    })
                    .collect()
            }
            None => list
                .split(',')
                .filter_map(|v| v.trim().parse::<usize>().ok())
                .filter(|&i| i < net.lanes.len())
                .collect(),
        };
        for i in chosen {
            let l = &net.lanes[i];
            let samples: Vec<String> = (0..l.points.len())
                .step_by((l.points.len() / 8).max(1))
                .map(|k| {
                    format!(
                        "[{:.1} m h {:.1} k {:.3}]",
                        l.dist[k],
                        l.headings[k],
                        l.curvature.get(k).copied().unwrap_or(0.0)
                    )
                })
                .collect();
            log::info!("lane {i}: {} {:?} rev {} turn {} prio {} len {:.1} start ({:.1}, {:.1}) end ({:.1}, {:.1}) next {:?} light {:?} crossings {:?} {}", l.name, l.key, l.reversed, l.turn, l.priority, l.length(), l.start().x, l.start().y, l.end().x, l.end().y, l.next, l.traffic_light, net.crossings.get(i), samples.join(" "));
        }
    }
}
