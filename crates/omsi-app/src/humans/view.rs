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
}
