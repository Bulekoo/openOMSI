//! The shadow maps' passes: the sun's cascades and the street lamps' tiles.

use super::*;

/// What a lamp's shadow tile draws this frame, summed up (`lamp_item_hash` of each item
/// added: the same whatever order the worker threads find them in), and which tiles draw
/// something that moves every frame (foliage bending in the wind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LampSigs {
    pub sums: [u64; LAMP_SHADOWS],
    pub volatile: u64,
}

impl Default for LampSigs {
    fn default() -> Self {
        LampSigs { sums: [0; LAMP_SHADOWS], volatile: 0 }
    }
}

impl LampSigs {
    pub(crate) fn add(&mut self, k: usize, h: u64) {
        self.sums[k] = self.sums[k].wrapping_add(h);
    }

    pub(crate) fn merge(&mut self, o: &LampSigs) {
        for k in 0..LAMP_SHADOWS {
            self.add(k, o.sums[k]);
        }
        self.volatile |= o.volatile;
    }
}

/// The lamps' shadow tiles as kept between frames. The lamps stand still, and so do
/// most things under them: every tile was cleared and drawn anew every night frame, and the
/// pass loaded and stored the whole far map with them (84 MB each way at the 4096 setting).
/// A tile is drawn again when what it draws has changed (an item, its place, a skinned
/// pose), its lamp's matrix has (another lamp in the tile, the render origin moved), or it
/// holds foliage bending in the wind; a frame without changed tiles has no lamp pass.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LampTiles {
    /// This frame's (set by `plan_shadow_casters`).
    pub frame: LampSigs,
    /// What each tile holds (the key of `lamp_tile_key`), `None`: nothing to keep.
    pub drawn: [Option<u64>; LAMP_SHADOWS],
}

impl Default for LampTiles {
    fn default() -> Self {
        LampTiles { frame: LampSigs::default(), drawn: [None; LAMP_SHADOWS] }
    }
}

fn mix(h: u64, x: u64) -> u64 {
    (h.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95)
}

/// One draw of a lamp tile, as `LampSigs` sums them: the mesh, its range and pose, the
/// material (and the slot's alpha, a script's fading part) and the instance's place. A saloon
/// lamp's tile (`frame`: the world's frame in the vehicle's, and the render origin) takes the
/// place in the vehicle's frame, to a coarser step: the numbers that come out of the vehicle's
/// own rotation are the same to a few millionths only.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lamp_item_hash(inst: &Instance, pose: u32, kind: u8, range: u32, material: u32, look: u32, alpha: f32, frame: Option<(&Mat4, DVec3)>) -> u64 {
    let mut h = mix(0x9e37_79b9_7f4a_7c15, inst.mesh as u64);
    h = mix(h, alpha.to_bits() as u64);
    h = mix(h, (range as u64) << 32 | pose as u64);
    h = mix(h, (material as u64) << 8 | kind as u64);
    h = mix(h, look as u64);
    if let Some((to_local, ro)) = frame {
        // (a few centimetres and a degree or so: a texel of a saloon's tile is three)
        let t = *to_local * Mat4::from_translation((inst.origin - ro).as_vec3()) * inst.transform;
        for (k, v) in t.to_cols_array().iter().enumerate() {
            let step = if k >= 12 { 0.05 } else { 0.02 };
            h = mix(h, (v / step).round() as i64 as u64);
        }
        return mix(h, 0x2545_f491_4f6c_dd1d);
    }
    // (the place to a few millimetres and the turn to a tenth of a degree, a fraction of
    // a tile's texel: a bus standing with its engine running shakes by less, and taken
    // exactly, every tile it stood in was drawn anew every frame)
    let t = inst.transform.to_cols_array();
    for (k, v) in t.iter().enumerate() {
        let step = if k >= 12 { 0.004 } else { 0.0015 };
        h = mix(h, (v / step).round() as i64 as u64);
    }
    for v in inst.origin.to_array() {
        h = mix(h, (v / 0.004).round() as i64 as u64);
    }
    // (a final mix: the sums of plain products would let two changes cancel too easily)
    mix(h, 0x2545_f491_4f6c_dd1d)
}

/// What tile `k` holds when drawn this frame for `lamp`.
pub(crate) fn lamp_tile_key(sums: &LampSigs, k: usize, view_proj: Mat4) -> u64 {
    let mut h = mix(sums.sums[k], k as u64);
    for v in view_proj.to_cols_array() {
        h = mix(h, v.to_bits() as u64);
    }
    h
}

impl Renderer {
    /// The near cascade (with the close one beside it in the atlas) and the far cascade,
    /// each when it is redrawn this frame.
    pub(crate) fn encode_shadow_maps(&self, shadow_encoder: &mut wgpu::CommandEncoder, scene: &Scene, sh: &ShadowPlan, shadow_batches: &[Vec<Batch>; SHADOW_LISTS], timers: &mut PassTimers) {
        let (draw_shadows, redraw_near, redraw_far) = (sh.draw_shadows, sh.redraw_near, sh.redraw_far);
        for cascade in [0usize, 1] {
            if !draw_shadows || (cascade == 1 && !redraw_far) {
                continue;
            }
            let view = if cascade == 0 {
                &self.shadows.shadow_view
            } else {
                &self.shadows.shadow_view_far
            };
            let keep_near = cascade == 0 && !redraw_near;
            // (the far map keeps the street lamps' tiles under it when they are kept)
            let keep_lamps = cascade == 1 && self.shadows.lamp_tiles.get().drawn.iter().any(|d| d.is_some());
            let mut pass = shadow_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: if keep_near || keep_lamps { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(1.0) },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: pass_timer(
                    timers.set.as_ref(),
                    &mut timers.timed,
                    if cascade == 0 {
                        "shadow near"
                    } else {
                        "shadow far"
                    },
                ),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, scene.shadow_bind_group.as_ref().unwrap(), &[]);
            if cascade == 0 {
                // the atlas: near cascade on the left half, close cascade on the right
                let sz = self.options.shadow_size as f32;
                pass.set_viewport(0.0, 0.0, sz, sz, 0.0, 1.0);
                encode_batches(&mut pass, scene, &shadow_batches[0], |pipe| {
                    &self.shadows.shadow_pipelines[pipe as usize]
                });
                let csz = self.options.shadow_size.min(SHADOW_CLOSE_MAX) as f32;
                pass.set_viewport(sz, 0.0, csz, csz, 0.0, 1.0);
                if keep_near {
                    // (the near half is last frame's: only the close part is cleared)
                    pass.set_pipeline(&self.shadows.shadow_clear_pipeline);
                    pass.draw(0..3, 0..1);
                    pass.set_bind_group(0, scene.shadow_bind_group.as_ref().unwrap(), &[]);
                }
                encode_batches(&mut pass, scene, &shadow_batches[2], |pipe| {
                    &self.shadows.shadow_pipelines[4 + pipe as usize]
                });
            } else {
                let sz = self.options.shadow_size as f32;
                pass.set_viewport(0.0, 0.0, sz, sz, 0.0, 1.0);
                if keep_lamps {
                    pass.set_pipeline(&self.shadows.shadow_clear_pipeline);
                    pass.draw(0..3, 0..1);
                    pass.set_bind_group(0, scene.shadow_bind_group.as_ref().unwrap(), &[]);
                }
                encode_batches(&mut pass, scene, &shadow_batches[cascade], |pipe| {
                    &self.shadows.shadow_pipelines[cascade * 2 + pipe as usize]
                });
            }
        }
    }

    // the lamps' maps: tiles under the far map (`LampAtlas`), each cleared and drawn when
    // what it shows has changed (see `LampTiles`)
    pub(crate) fn encode_lamp_shadows(&self, shadow_encoder: &mut wgpu::CommandEncoder, scene: &Scene, f: &FrameCtx, shadow_batches: &[Vec<Batch>; SHADOW_LISTS], timers: &mut PassTimers) {
        let lamp_shadows = &f.lamp_shadows;
        // (the frames that draw the lamps' maps keep the record: a mirror's has none)
        if !f.with_overlays || f.projection.is_some() {
            return;
        }
        let mut tiles = self.shadows.lamp_tiles.get();
        let mut redraw = [false; LAMP_SHADOWS];
        for (k, d) in tiles.drawn.iter_mut().enumerate() {
            let Some(l) = lamp_shadows.iter().find(|l| l.slot == k) else {
                *d = None;
                continue;
            };
            let key = lamp_tile_key(&tiles.frame, k, l.key_matrix());
            if *d != Some(key) || tiles.frame.volatile & (1u64 << k) != 0 || f.env.lamp_shadows_every_frame {
                if f.env.debug_draws {
                    log::info!("lamp shadow tile {k} drawn: {} (lamp at {:?}, {} batches)", if *d != Some(key) { "changed" } else { "moving foliage" }, l.position, shadow_batches[3 + k].len());
                }
                redraw[k] = true;
                *d = Some(key);
            }
        }
        self.shadows.lamp_tiles.set(tiles);
        if f.env.debug_draws {
            *self.counts.borrow_mut().entry("lamp shadow tiles drawn").or_default() += redraw.iter().filter(|r| **r).count() as f64;
        }
        if redraw.iter().any(|r| *r) {
            let mut pass = shadow_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lamp shadows"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadows.shadow_view_far,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: pass_timer(timers.set.as_ref(), &mut timers.timed, "lamp shadows"),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let atlas = self.shadows.lamp_atlas;
            let tile = atlas.tile as f32;
            for k in (0..LAMP_SHADOWS).filter(|k| redraw[*k]) {
                let (x, y) = atlas.origin(k);
                pass.set_viewport(x as f32, y as f32, tile, tile, 0.0, 1.0);
                pass.set_pipeline(&self.shadows.shadow_clear_pipeline);
                pass.draw(0..3, 0..1);
                pass.set_bind_group(0, scene.shadow_bind_group.as_ref().unwrap(), &[]);
                encode_batches(&mut pass, scene, &shadow_batches[3 + k], |pipe| {
                    &self.shadows.shadow_pipelines[6 + pipe as usize]
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tiles' sums do not depend on the order the workers find the items in, and a
    /// lamp's tile key changes with its matrix and its place among the tiles.
    #[test]
    fn lamp_tile_keys() {
        let items = [3u64, 0xdead_beef, 77, 1 << 40];
        let (mut a, mut b, mut c) = (LampSigs::default(), LampSigs::default(), LampSigs::default());
        for &h in &items {
            a.add(1, h);
        }
        for &h in items.iter().rev().skip(2) {
            b.add(1, h);
        }
        for &h in items.iter().rev().take(2) {
            c.add(1, h);
        }
        b.merge(&c);
        assert_eq!(a, b);
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let k = lamp_tile_key(&a, 1, m);
        assert_eq!(k, lamp_tile_key(&b, 1, m));
        assert_ne!(k, lamp_tile_key(&a, 2, m));
        assert_ne!(k, lamp_tile_key(&a, 1, Mat4::IDENTITY));
        a.add(1, 5);
        assert_ne!(k, lamp_tile_key(&a, 1, m));
    }
}
