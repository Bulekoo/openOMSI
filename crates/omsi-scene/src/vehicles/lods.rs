//! The lower levels of detail of a vehicle (`[LOD]` of its model): their meshes on the
//! graphics card with plain materials, and their instances, drawn in place of the full model
//! while the vehicle is small on the screen (the AI traffic's, see `add_vehicle_shared_lod`).

use super::*;

/// Where the keys of the lower levels' meshes start in a vehicle's mesh cache (the full model's
/// meshes are keyed by their index).
pub(super) const FAR_MESH_KEY: usize = 1 << 20;

/// The size band each level of detail is drawn in, by the object's screen size: OMSI picks the
/// first level, in the order the model lists them, whose least size the object reaches, else
/// the last one whatever its own (Omsi.exe 0x5ef860, as the scenery's levels, see
/// `type_gpu_lods`): a level is drawn from its least size (the last from 0) up to the least of
/// the sizes listed before it.
pub(super) fn lod_bands(model: &omsi_model::Model) -> Vec<(f32, f32)> {
    let mins: Vec<f32> = model.lods.iter().map(|l| l.min_size).collect();
    (0..mins.len())
        .map(|i| {
            let lo = if i + 1 == mins.len() { 0.0 } else { mins[i] };
            (lo, mins[..i].iter().copied().fold(f32::MAX, f32::min))
        })
        .collect()
}

impl World {
    /// The lower levels' meshes of `vt` and their plain materials, for a vehicle set being
    /// uploaded (`skip`: a debugging filter keeps only some meshes: none of these).
    pub(super) fn upload_far(
        &self,
        renderer: &Renderer,
        scene: &mut Scene,
        up: &mut VehicleUpload,
        mesh_ids: &mut HashMap<(PathBuf, usize), (MeshId, usize)>,
        mesh_keys: &mut Vec<(PathBuf, usize)>,
        skip: bool,
    ) -> Vec<FarMesh> {
        let vt = up.vt;
        let mut far = Vec::new();
        for (k, vm) in vt.lod_meshes.iter().enumerate() {
            if skip {
                break;
            }
            let def = &vt.model.meshes[vm.def_index];
            let mats: Vec<MaterialId> = vm
                .materials
                .iter()
                .enumerate()
                .map(|(slot, m)| self.far_material(renderer, scene, up, vm, slot, m))
                .collect();
            let key = (vt.def.path.clone(), FAR_MESH_KEY + k);
            let id = match mesh_ids.get_mut(&key) {
                Some(e) => {
                    if !mesh_keys.contains(&key) {
                        e.1 += 1;
                        mesh_keys.push(key);
                    }
                    e.0
                }
                None => {
                    let id = self.gpu.lock().add_mesh(renderer, scene, &vm.data);
                    scene.meshes[id].source = Some(vt.def.path.display().to_string());
                    mesh_ids.insert(key.clone(), (id, 1));
                    mesh_keys.push(key);
                    id
                }
            };
            far.push(FarMesh { level: def.lod, mesh: id, mats, casts: def.shadow });
        }
        if omsi_cfg::flags::OMSI_PROFILE.is_set() && !vt.lod_meshes.is_empty() {
            log::info!("  vehicle set {}: {} meshes of {} lower levels of detail ({:?})", vt.def.path.file_name().unwrap_or_default().to_string_lossy(), far.len(), vt.model.lods.len().saturating_sub(1), lod_bands(&vt.model));
        }
        far
    }

    /// The instances of a vehicle's lower levels of detail, and the size bands of the full
    /// model's (`instances`): as Omsi.exe picks a level by the vehicle's size on the screen,
    /// the full model for a big one and the lower levels' meshes (plain, still, standing
    /// where the body does) for a small one - a bus far down the street was every one of its
    /// 200 meshes, tens of thousands of triangles, for a handful of pixels. None are made
    /// unless `on` (a vehicle whose pose the caller gives them too).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn far_instances(
        &self,
        renderer: &Renderer,
        scene: &mut Scene,
        vt: &omsi_sim::VehicleType,
        set: &VehicleSet,
        instances: &[usize],
        on: bool,
        gpu: &mut GpuCache,
    ) -> Vec<usize> {
        let mut bands = lod_bands(&vt.model);
        // (OMSI_FORCE_LOD=1, to look at the lower levels: they are drawn at any size)
        if omsi_cfg::flags::OMSI_FORCE_LOD.is_set() && bands.len() > 1 {
            bands = std::iter::once((f32::MAX * 0.5, f32::MAX)).chain(bands.iter().skip(1).map(|_| (0.0, f32::MAX))).collect();
        }
        let mut far = Vec::new();
        if on && !set.far.is_empty() && bands.len() > 1 {
            for &inst in instances {
                if scene.instances.get(inst).is_some_and(|x| !x.blob) {
                    renderer.set_lod_range(scene, inst, bands[0].0, bands[0].1);
                }
            }
            renderer.begin_shared_object(scene, (&vt.def.path, "far"));
            for f in &set.far {
                let (lo, hi) = bands.get(f.level).copied().unwrap_or((0.0, f32::MAX));
                let i = renderer.add_instance(scene, f.mesh, DVec3::ZERO, Mat4::IDENTITY, f.mats.clone());
                renderer.set_omsi_caster(scene, i, f.casts);
                renderer.set_roof(scene, i, vt.def.bounding_box.map(|b| b[5] + b[2] * 0.5));
                renderer.set_lod_range(scene, i, lo, hi);
                far.push(gpu.instance(renderer, scene, i));
            }
        }
        far
    }

    /// The plain material of one slot of a lower level of detail's mesh: its texture (in the
    /// paint scheme's pictures) and its alpha mode, nothing the scripts switch.
    pub(super) fn far_material(
        &self,
        renderer: &Renderer,
        scene: &mut Scene,
        up: &mut VehicleUpload,
        vm: &omsi_sim::vehicle::VehicleMesh,
        slot: usize,
        m: &omsi_o3d::Material,
    ) -> MaterialId {
        let name = up
            .subst
            .get(&m.texture.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| m.texture.clone());
        let tex = if is_null_texture(&m.texture) || name.trim().is_empty() || name.starts_with("\\S:") {
            None
        } else {
            self.vehicle_texture(renderer, scene, &mut up.tex_ids, &mut up.held, name.trim(), up.dirs_ref)
        };
        let mat = renderer.add_material_night(scene, tex, material_alpha(&vm.materials, slot, &vm.overrides), [1.0; 4], false, None, None);
        let mat = self.gpu.lock().material(renderer, scene, mat);
        up.materials.push(mat);
        mat
    }

}
