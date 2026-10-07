//! The people as the renderer sees them: their meshes and instances, the GPU side of the
//! human types, the posing and skinning, and the coins on the cash desk.
//!
//! The simulation does not touch the renderer. What it does that the renderer has to
//! follow - somebody appearing, somebody going, coins put on the desk - it writes down as
//! [`BodyOp`]s, and the view replays them in the order they happened (`show_bodies`), at
//! the end of the same call that made them: the renderer sees the very calls, in the very
//! order, it saw when the simulation made them itself.

use super::*;

/// Something the simulation did that the renderer has to follow.
pub(super) enum BodyOp {
    /// Somebody appeared: the meshes of their type and clothes, at `position`.
    Spawn { id: u32, ty: Arc<HumanType>, variant: usize, position: DVec3 },
    /// Somebody went: hidden, and their meshes kept for the next person of the type.
    /// `meshes` are theirs as they were when they went (none yet for somebody who came
    /// and went before the view caught up).
    Retire { id: u32, tkey: usize, variant: usize, meshes: Vec<(MeshId, usize)> },
    /// Coins on a point of the player's cabin (`Money::place`).
    Coins { coins: Vec<usize>, point: Vec3, var: [f32; 2], change: bool, parent: Option<String> },
}

impl BodyOp {
    /// Whether replaying it needs the world (textures, the money's meshes).
    fn needs_world(&self) -> bool {
        !matches!(self, BodyOp::Retire { .. })
    }
}

/// The renderer's side of the people.
pub(super) struct Bodies {
    /// What the simulation did since the view last caught up, oldest first.
    pub(super) ops: Vec<BodyOp>,
    pub(super) hidden: Vec<usize>,
    /// GPU side of the human types, shared by everyone of a type: textures by file and the
    /// materials of every (type, mesh) - each person used to upload its own copies - and
    /// the meshes and instances of the people who have gone, taken over by the next person
    /// of the same type (the skinned vertices are rewritten anyway). Without that every
    /// passenger who ever appeared kept a mesh, its textures and materials on the GPU.
    pub(super) gpu_textures: HashMap<PathBuf, Option<omsi_render::TextureId>>,
    /// Per (type, clothing variant, mesh): its materials, and the meshes and instances of
    /// people who have gone, kept for the next person dressed alike.
    pub(super) gpu_materials: HashMap<(usize, usize, usize), Vec<MaterialId>>,
    pub(super) spare: HashMap<(usize, usize, usize), Vec<(MeshId, usize)>>,
    pub(super) sync_frame: u32,
    /// Simulation time of the last `sync`.
    pub(super) last_sync: f64,
    /// Frames synced, people posed and skinned, the time that took and the part of it spent
    /// uploading (ms), in total.
    pub(super) pose_stats: (u32, usize, f64, f64),
    /// `OMSI_TRACE_PAX=<csv>`: every person near the eye, every frame (see `sync`).
    pub(super) trace: Option<std::io::BufWriter<std::fs::File>>,
}

impl Bodies {
    pub(super) fn new() -> Bodies {
        Bodies {
            ops: Vec::new(),
            hidden: Vec::new(),
            gpu_textures: HashMap::new(),
            gpu_materials: HashMap::new(),
            spare: HashMap::new(),
            sync_frame: 0,
            last_sync: 0.0,
            pose_stats: (0, 0, 0.0, 0.0),
            trace: omsi_cfg::env::var("OMSI_TRACE_PAX").ok().and_then(|f| std::fs::File::create(f).ok()).map(|f| {
                use std::io::Write;
                let mut w = std::io::BufWriter::new(f);
                let _ = writeln!(w, "t,id,state,ground,posed,x,y,z,heading,lx,ly,lz,rx,ry,rz,vx,vy");
                w
            }),
        }
    }

    /// Somebody not yet drawn is now called `to` (a LAN host's person takes the host's id).
    pub(super) fn renamed(&mut self, from: u32, to: u32) {
        for op in self.ops.iter_mut() {
            if let BodyOp::Spawn { id, .. } = op {
                if *id == from {
                    *id = to;
                }
            }
        }
    }
}

impl Humans {
    /// Somebody has gone: hidden, and their meshes kept for the next person of the type.
    pub(super) fn retire(&mut self, p: &Person) {
        let tkey = Arc::as_ptr(&p.ty) as usize;
        self.bodies.ops.push(BodyOp::Retire { id: p.id, tkey, variant: p.variant, meshes: p.meshes.clone() });
    }

    /// Bring the renderer up to what the simulation did (see [`BodyOp`]).
    pub(super) fn show_bodies(&mut self, world: &World, renderer: &Renderer, scene: &mut Scene) {
        self.replay_bodies(Some(world), renderer, scene);
    }

    /// Bring the renderer up to the people who went since (the frame's `sync`; whatever
    /// made people or coins has shown them already).
    pub(super) fn catch_up_bodies(&mut self, renderer: &Renderer, scene: &mut Scene) {
        self.replay_bodies(None, renderer, scene);
    }

    /// Replay the ops in order: up to the first that needs the world when there is none.
    fn replay_bodies(&mut self, world: Option<&World>, renderer: &Renderer, scene: &mut Scene) {
        if self.bodies.ops.is_empty() {
            return;
        }
        let ops = std::mem::take(&mut self.bodies.ops);
        // the meshes of people who came and went before the view caught up
        let mut gone: HashMap<u32, Vec<(MeshId, usize)>> = HashMap::new();
        let mut ops = ops.into_iter();
        while let Some(op) = ops.next() {
            let world = match world {
                Some(w) => w,
                None if op.needs_world() => {
                    // (cannot happen: whatever makes such ops shows them before it returns)
                    log::warn!("people: {} body changes left for later", ops.len() + 1);
                    self.bodies.ops = std::iter::once(op).chain(ops).collect();
                    return;
                }
                None => {
                    let BodyOp::Retire { id, tkey, variant, meshes } = op else { unreachable!() };
                    self.retire_meshes(id, tkey, variant, meshes, &mut gone);
                    continue;
                }
            };
            match op {
                BodyOp::Spawn { id, ty, variant, position } => {
                    let meshes = self.make_meshes(world, renderer, scene, &ty, variant, position);
                    match self.people.iter_mut().find(|p| p.id == id) {
                        Some(p) => p.meshes = meshes,
                        None => {
                            gone.insert(id, meshes);
                        }
                    }
                }
                BodyOp::Retire { id, tkey, variant, meshes } => self.retire_meshes(id, tkey, variant, meshes, &mut gone),
                BodyOp::Coins { coins, point, var, change, parent } => {
                    if let Some(m) = self.money.as_mut() {
                        m.place(world, renderer, scene, &coins, point, var, change, parent.as_deref());
                    }
                }
            }
        }
    }

    fn retire_meshes(&mut self, id: u32, tkey: usize, variant: usize, meshes: Vec<(MeshId, usize)>, gone: &mut HashMap<u32, Vec<(MeshId, usize)>>) {
        let meshes = gone.remove(&id).unwrap_or(meshes);
        for (mi, m) in meshes.iter().enumerate() {
            self.bodies.hidden.push(m.1);
            self.bodies.spare.entry((tkey, variant, mi)).or_default().push(*m);
        }
    }

    /// The meshes and instances of somebody new: those of somebody gone dressed alike, else
    /// made afresh (with the type's materials, made once per type and clothes).
    fn make_meshes(&mut self, world: &World, renderer: &Renderer, scene: &mut Scene, ty: &Arc<HumanType>, variant: usize, position: DVec3) -> Vec<(MeshId, usize)> {
        let tkey = Arc::as_ptr(ty) as usize;
        let mut meshes = Vec::new();
        for (mi, hm) in ty.meshes.iter().enumerate() {
            let key = (tkey, variant, mi);
            // somebody of this type has gone: their mesh and instance
            if let Some((id, inst)) = self.bodies.spare.get_mut(&key).and_then(|v| v.pop()) {
                self.bodies.hidden.retain(|h| *h != inst);
                renderer.set_transform(scene, inst, position, Mat4::IDENTITY);
                renderer.set_params(scene, inst, &[], true, &[]);
                meshes.push((id, inst));
                continue;
            }
            if !self.bodies.gpu_materials.contains_key(&key) {
                let dirs = ty.texture_dirs(&world.root);
                let mut mats = Vec::new();
                for (k, m) in hm.materials.iter().enumerate() {
                    // the variant's texture from its own folder first, else the default
                    let (name, first) = ty.variant_texture(&m.texture, variant);
                    let mut look: Vec<&Path> = first.into_iter().collect();
                    look.extend(dirs.iter().map(|p| p.as_path()));
                    let found = omsi_texture::find_texture(name, &look)
                        .or_else(|| omsi_texture::find_texture(&m.texture, &look));
                    if found.is_none() && !m.texture.trim().is_empty() {
                        log::warn!(
                            "human {}: texture {} not found",
                            ty.def.path.display(),
                            m.texture
                        );
                    }
                    let tex = match found {
                        Some(path) => match self.bodies.gpu_textures.get(&path) {
                            Some(t) => *t,
                            None => {
                                let t = world
                                    .textures
                                    .get_gpu_fast(&path)
                                    .map(|(img, _)| renderer.add_texture_data(scene, &img));
                                world.textures.release(&path);
                                self.bodies.gpu_textures.insert(path, t);
                                t
                            }
                        },
                        None => None,
                    };
                    let alpha = match hm.alpha.get(k).copied().unwrap_or(0) {
                        1 => AlphaMode::Test,
                        2 => AlphaMode::Blend,
                        _ => AlphaMode::Opaque,
                    };
                    mats.push(renderer.add_material(scene, tex, alpha, [1.0; 4], false));
                }
                self.bodies.gpu_materials.insert(key, mats);
            }
            let mats = self.bodies.gpu_materials[&key].clone();
            let id = renderer.add_mesh(scene, &hm.data);
            let inst = renderer.add_instance(scene, id, position, Mat4::IDENTITY, mats);
            meshes.push((id, inst));
        }
        meshes
    }

    /// OMSI's `change_take`: the driver takes back the coins lying on the change tray.
    pub fn take_change_tray(&mut self) {
        if let Some(m) = self.money.as_mut() {
            m.clear(true);
        }
    }

    /// Coins the driver handed out (from the host's GiveChangeCoin list) onto the change point.
    pub fn give_change(
        &mut self,
        world: &World,
        renderer: &Renderer,
        scene: &mut Scene,
        coins: &[usize],
    ) {
        if coins.is_empty() {
            return;
        }
        let point = self.player_cabin.as_ref().and_then(|c| {
            c.data
                .change_points
                .first()
                .or(c.data.money_points.first())
                .cloned()
        });
        if let (Some(m), Some(pt)) = (self.money.as_mut(), point) {
            m.place(
                world,
                renderer,
                scene,
                coins,
                Vec3::from(pt.pos),
                pt.var,
                true,
                pt.parent.as_deref(),
            );
        }
    }

    pub fn sync_money(&mut self, world: &World, renderer: &Renderer, scene: &mut Scene, bus: &VehicleInstance) {
        if let Some(m) = self.money.as_mut() {
            m.sync(renderer, scene, bus);
        }
        if let Some(pack) = self.tickets.as_ref().map(|t| t.path.clone()) {
            let fresh = self.ticket_blocks.as_ref().is_none_or(|b| b.made_for != (bus.ty.def.path.clone(), pack.clone()));
            if fresh && !bus.ty.def.attachments.is_empty() {
                self.ticket_blocks = Some(crate::money::TicketBlocks::new(world, renderer, scene, bus, &pack));
            }
        }
        if let Some(b) = self.ticket_blocks.as_ref() {
            b.sync(renderer, scene, bus);
        }
    }

    /// Skin the people due for a new pose and push transforms to the renderer. Near people
    /// are posed every frame, far ones every few frames and people out of view rarely; the
    /// posing and skinning run in parallel.
    pub fn sync(&mut self, renderer: &Renderer, scene: &mut Scene, camera: DVec3) {
        self.catch_up_bodies(renderer, scene);
        for inst in self.bodies.hidden.drain(..) {
            renderer.set_params(scene, inst, &[], false, &[]);
        }
        let started = std::time::Instant::now();
        self.bodies.sync_frame = self.bodies.sync_frame.wrapping_add(1);
        let eye = self.eye;
        let from = eye.map(|e| e.pos).unwrap_or(camera);
        // synced only now and then (offscreen snapshots): everybody is posed afresh
        let all = self.time - self.bodies.last_sync > 0.12;
        let sdt = (self.time - self.bodies.last_sync).clamp(0.0, 0.5) as f32;
        self.bodies.last_sync = self.time;
        let mut due: Vec<bool> = Vec::with_capacity(self.people.len());
        for (k, p) in self.people.iter_mut().enumerate() {
            p.since_posed = p.since_posed.saturating_add(1);
            let d = p.position + DVec3::Z * 0.9 - from;
            let dist = d.length();
            let visible = match eye {
                Some(e) => dist < 4.0 || d.dot(e.fwd) / dist.max(1e-3) > e.cos_half - 0.15,
                None => true,
            };
            // everybody the eye can make out is posed every frame: a pose every other
            // frame at 12-30 m moved walkers in steps and made planted feet shiver
            // (within 30 m everybody, seen or not: the mirrors show the people behind the
            // bus, who were posed every twelfth frame and moved in jerks there)
            let every = if dist < 30.0 {
                1
            } else if !visible {
                12
            } else if dist < 45.0 {
                1
            } else if dist < 90.0 {
                2
            } else if dist < 160.0 {
                3
            } else {
                6
            };
            let every = if p.vel.length_squared() < 1e-4 && dist > 20.0 { every * 2 } else { every };
            // spread the far ones over the frames
            let turn = (self.bodies.sync_frame + k as u32) % every == 0;
            due.push(
                !p.skinned
                    || all
                    || (p.since_posed >= every && (turn || p.since_posed >= 2 * every)),
            );
        }
        let n_due = due.iter().filter(|d| **d).count();
        let pose_one = |p: &mut Person| {
            let Person { anim, ty, skins, skin_bones, pose_changed, .. } = p;
            *pose_changed = false;
            let bones = omsi_sim::human::slots_from_omsi(&anim.bones(&ty.omsi));
            if bones.iter().any(|b| !b.is_finite()) && !skins.is_empty() {
                // keep the last good mesh (the rest pose would be the file's T-pose)
                return;
            }
            // (the same bones as the mesh was made with: nothing to skin or upload)
            if skins.len() == ty.meshes.len() && skin_bones.as_ref().is_some_and(|b| b.iter().zip(&bones).all(|(a, c)| a.abs_diff_eq(*c, 1e-6))) {
                return;
            }
            skins.resize_with(ty.meshes.len(), Default::default);
            for (k, m) in ty.meshes.iter().enumerate() {
                let (pos, nrm) = &mut skins[k];
                skin(m, &bones, pos, nrm);
            }
            *skin_bones = Some(bones);
            *pose_changed = true;
        };
        // a handful is quicker on this thread than handed to the pool
        if n_due >= 8 {
            self.people
                .par_iter_mut()
                .zip(due.par_iter())
                .with_min_len(2)
                .filter(|(_, go)| **go)
                .for_each(|(p, _)| pose_one(p));
        } else {
            self.people
                .iter_mut()
                .zip(&due)
                .filter(|(_, go)| **go)
                .for_each(|(p, _)| pose_one(p));
        }
        let upload = std::time::Instant::now();
        for (p, &go) in self.people.iter_mut().zip(&due) {
            if go {
                if p.pose_changed || !p.skinned {
                    for (k, (id, _)) in p.meshes.iter().enumerate() {
                        if let Some((pos, nrm)) = p.skins.get(k) {
                            renderer.update_mesh(scene, *id, pos, nrm, &p.ty.meshes[k].data.uvs);
                        }
                    }
                }
                p.skinned = true;
                p.since_posed = 0;
                p.posed_at = (p.position, p.heading);
            }
            // riders go with their bus; on the ground a mesh not posed this frame goes on
            // with the body too (left where it was posed, a far walker moved in jerks -
            // its feet slide a few centimetres instead, which nobody sees at that distance)
            let (at, heading) = match (p.puppet, p.place) {
                (_, Place::Ground) if go => p.posed_at,
                _ => (p.position, p.heading),
            };
            // (riders with the tilt of their floor)
            let tilt = if matches!(p.place, Place::Bus(..)) { p.tilt } else { Mat4::IDENTITY };
            let xf = tilt * Mat4::from_rotation_z((-heading).to_radians() as f32);
            let lit_to = if matches!(p.place, Place::Bus(..)) { p.interior } else { 0.0 };
            p.lit += (lit_to - p.lit) * (sdt / 0.4).min(1.0);
            for (_, inst) in &p.meshes {
                renderer.set_transform(scene, *inst, at, xf);
                renderer.set_interior(scene, *inst, p.lit * 0.5);
            }
            if self.avatar_hidden.contains_key(&p.id) && omsi_cfg::env::var_os("OMSI_DEBUG_FOOT").is_some() && self.bodies.sync_frame % 30 == 0 {
                log::info!("avatar drawn at ({:.2}, {:.2}, {:.2}) heading {:.0} place {:?} go {}", at.x, at.y, at.z, heading, matches!(p.place, Place::Ground), go);
            }
            if let Some(hide) = self.avatar_hidden.get_mut(&p.id) {
                // (the first-person view: the avatar's own body out of the picture; set
                // every frame, the posing would show it again)
                for (_, inst) in &p.meshes {
                    renderer.set_params(scene, *inst, &[], !*hide, &[]);
                }
            }
            if let Some(t) = self.bodies.trace.as_mut() {
                // OMSI_TRACE_PAX: where the mesh is drawn and where its ankles are, per frame
                if (at - from).length() < 40.0 {
                    use std::io::Write;
                    let a = |k: usize| at + (xf.transform_vector3(p.ankles[k])).as_dvec3();
                    let (l, r) = (a(0), a(1));
                    let _ = writeln!(
                        t,
                        "{:.4},{},{},{},{},{:.4},{:.4},{:.4},{:.2},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.3},{:.3}",
                        self.time,
                        p.id,
                        p.state.name(),
                        matches!(p.place, Place::Ground) as u8,
                        go as u8,
                        at.x,
                        at.y,
                        at.z,
                        heading,
                        l.x,
                        l.y,
                        l.z,
                        r.x,
                        r.y,
                        r.z,
                        p.vel.x,
                        p.vel.y
                    );
                }
            }
        }
        // OMSI_CHECK_TPOSE=1: everybody drawn with the arms out (the file's rest pose): the
        // skinned mesh wider than 1.3 m from hand to hand
        if omsi_cfg::env::var_os("OMSI_CHECK_TPOSE").is_some() {
            for p in &self.people {
                let Some((pos, _)) = p.skins.first() else {
                    log::info!("t-pose? {} {}: never skinned", p.label(), p.state_name());
                    continue;
                };
                let (lo, hi) = pos.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v.x), hi.max(v.x)));
                if hi - lo > 1.3 {
                    let pax = match &p.state {
                        State::Pax(x) => format!("pax_state {} speed {:.2} seat_h {:.2} room {:.2} st {}", x.pax_state, x.speed, x.seat_h, x.room, x.st),
                        _ => String::new(),
                    };
                    log::info!("t-pose: {} {} width {:.2} skinned {} {pax}", p.label(), p.state_name(), hi - lo, p.skinned);
                }
            }
        }
        self.bodies.pose_stats.0 += 1;
        self.bodies.pose_stats.1 += n_due;
        self.bodies.pose_stats.2 += started.elapsed().as_secs_f64() * 1000.0;
        self.bodies.pose_stats.3 += upload.elapsed().as_secs_f64() * 1000.0;
    }
}
