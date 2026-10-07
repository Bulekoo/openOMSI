//! LAN play (see `lan_world`): a host keeps traffic around every player and tells the
//! clients its light programs; a client draws the host's cars instead of its own.

use super::*;
use crate::scene::World;
use omsi_render::{Renderer, Scene};

impl Traffic {
    pub fn is_mirror(&self) -> bool {
        self.mirror
    }

    /// Draw the host's traffic from now on (`on`), or simulate our own again: either way
    /// every car there is now goes (ours make room for the host's, the host's copies
    /// cannot drive on by themselves).
    pub fn set_mirror(&mut self, world: &World, renderer: &Renderer, scene: &mut Scene, on: bool) {
        if self.mirror == on {
            return;
        }
        self.mirror = on;
        if !on {
            let day_time = self.day_time;
            for ctl in &mut self.lights {
                reset_light_runtime(ctl, day_time);
            }
        }
        let ids: Vec<u64> = self.cars.iter().map(|c| c.id).collect();
        for id in ids {
            self.remove_car(world, renderer, scene, id);
        }
        self.initial = !on;
        log::info!(
            "traffic: {}",
            if on {
                "the LAN host's traffic is drawn instead of our own"
            } else {
                "simulating our own traffic again"
            }
        );
    }

    /// A client's frame: interpolate the host's light clocks, without re-evaluating its
    /// stop and jump points from the client's incomplete traffic requests.
    pub(super) fn mirror_tick(&mut self, dt: f32) {
        self.time += dt;
        self.day_time += dt as f64 * self.time_scale;
        self.last_dt = dt;
        let day_time = self.day_time;
        for c in self.lights.iter_mut() {
            mirror_light_tick(c, dt, day_time);
        }
        self.log_lights();
    }

    /// A car of the host's traffic, standing at `pos` (client). Its id is the host's.
    #[allow(clippy::too_many_arguments)]
    pub fn add_mirror_car(
        &mut self,
        world: &World,
        renderer: &Renderer,
        scene: &mut Scene,
        id: u64,
        ty: Arc<VehicleType>,
        scheme: Option<usize>,
        scheduled: bool,
        pos: DVec3,
        heading: f64,
    ) -> usize {
        let mut host = omsi_sim::VehicleHost::new(omsi_sim::SimClock::default());
        host.font_lib = Some(world.fonts.clone());
        let scheme = scheme.filter(|i| *i < ty.paint_schemes.len());
        host.paint_scheme = Some(scheme);
        let mut vehicle = VehicleInstance::new(ty.clone(), host);
        // (the host's poses say where it stands; nothing here pulls it onto the ground)
        vehicle.ground = None;
        vehicle.apply_paint_vars(scheme);
        let render = world.add_vehicle_shared(renderer, scene, &ty, scheme, None);
        let trailer_renders =
            self.attach_trailers(world, renderer, scene, &mut vehicle, scheme, &render);
        if !ty.model.text_textures.is_empty() {
            vehicle.init_text_textures(&mut world.fonts.lock(), &|p| {
                omsi_texture::decode_file(p)
                    .ok()
                    .map(|i| (i.width, i.height, i.rgba))
            });
        }
        vehicle.position = pos;
        vehicle.heading = heading;
        let (front, rear, half_width) = extents(&ty, 4.5);
        let mut state = AiState::new(0, 0.0, id);
        state.front = front;
        state.rear = rear;
        state.length = front + rear;
        let body = AiBody::new(&ty.def, MotionKind::Road);
        self.cars.push(AiCar {
            id,
            state,
            vehicle,
            render,
            trailer_renders,
            body,
            stopped: 0.0,
            lead_car: None,
            ignore_lead: None,
            crawl: 0.0,
            progress: (0.0, 0.0),
            bus: scheduled.then(|| Box::new(BusService::new(Vec::new()))),
            sounds: None,
            half_width,
            yielding: false,
            light_hold: false,
            reserved: Vec::new(),
            amber: None,
            passing: None,
            gone: false,
            fresh: 0.0,
            merge_after: None,
            holding: None,
            why: ("", 0.0),
            held: false,
            geo_block: None,
            lead_info: None,
            junction_why: String::new(),
            wait_at: None,
            seed: 0,
            scheme: None,
            squeeze: None,
            pass_room: 0.0,
            pass_retry: 0.0,
            light_at: None,
            pull_out: 0.0,
            rail_trail: Default::default(),
            ai_secs: 0.0,
            consist_reversed: false,
            park: None,
        });
        self.cars.len() - 1
    }

    /// A vehicle type the traffic has loaded already (the random traffic's types and the
    /// coupled parts), by file.
    pub fn loaded_type(&self, path: &Path) -> Option<Arc<VehicleType>> {
        self.types
            .iter()
            .map(|t| &t.0)
            .chain(self.trailer_types.values().flatten())
            .find(|t| t.def.path == path)
            .cloned()
    }
}
