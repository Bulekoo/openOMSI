//! The frame, phase by phase: `Renderer::render_inner` plans what is drawn (the shadow
//! cascades, the culling, the batches) and then encodes the passes in their order. What
//! the phases share is the frame's `FrameCtx`, built once at its start.

use super::*;

mod batch;
mod cull;
mod effects;
mod main_pass;
mod post;
mod prepass;
mod setup;
mod shadow;
mod shadow_plan;
mod submit;

pub(crate) use batch::DrawPlan;
pub(crate) use shadow_plan::ShadowPlan;

/// The frame's switches from the environment: read once at the start of every frame
/// (`omsi_cfg::env`), not wherever they are tested.
pub(crate) struct FrameEnv {
    /// `OMSI_FAKE_GPU_ERROR`: `lost` / `frame`, the device-loss and fallback test hooks
    pub fake_gpu_error: Option<String>,
    pub no_enhanced: bool,
    pub no_fxaa: bool,
    pub no_glass_picture: bool,
    pub mirror_enhanced: bool,
    pub no_puddle_reflections: bool,
    pub no_rt_frame: bool,
    pub no_ao: bool,
    pub shadow_near_every_frame: bool,
    pub shadow_far_every_frame: bool,
    pub debug_shadow_far: bool,
    pub debug_view_lamps: bool,
    pub debug_sky: bool,
    pub debug_draws: bool,
    /// `OMSI_DEBUG_CULL`: set at all, and its value (`x,y,radius`: the instances to dump)
    pub debug_cull: bool,
    pub debug_cull_at: Option<String>,
    /// `OMSI_DEBUG_SHADOW`: set at all, and its value (the smallest radius logged)
    pub debug_shadow: bool,
    pub debug_shadow_r: Option<String>,
    pub debug_flicker: bool,
    pub only_surfaces: bool,
    pub skip_pipe: Option<String>,
    pub no_bundles: bool,
    pub no_msaa_prepass: bool,
    pub no_main_split: bool,
    pub no_smoke: bool,
    pub no_snowfall: bool,
    pub no_coronas: bool,
    pub no_glare: bool,
    pub debug_fog_lamps: bool,
    pub no_rt_grade: bool,
}

impl FrameEnv {
    pub(crate) fn read() -> FrameEnv {
        let set = |name: &str| omsi_cfg::env::var_os(name).is_some();
        let value = |name: &str| omsi_cfg::env::var(name).ok();
        let debug_cull_at = omsi_cfg::env::var_os("OMSI_DEBUG_CULL");
        let debug_shadow_r = omsi_cfg::env::var_os("OMSI_DEBUG_SHADOW");
        FrameEnv {
            fake_gpu_error: value("OMSI_FAKE_GPU_ERROR"),
            no_enhanced: set("OMSI_NO_ENHANCED"),
            no_fxaa: set("OMSI_NO_FXAA"),
            no_glass_picture: set("OMSI_NO_GLASS_PICTURE"),
            mirror_enhanced: set("OMSI_MIRROR_ENHANCED"),
            no_puddle_reflections: set("OMSI_NO_PUDDLE_REFLECTIONS"),
            no_rt_frame: set("OMSI_NO_RT_FRAME"),
            no_ao: set("OMSI_NO_AO"),
            shadow_near_every_frame: set("OMSI_SHADOW_NEAR_EVERY_FRAME"),
            shadow_far_every_frame: set("OMSI_SHADOW_FAR_EVERY_FRAME"),
            debug_shadow_far: set("OMSI_DEBUG_SHADOW_FAR"),
            debug_view_lamps: set("OMSI_DEBUG_VIEW_LAMPS"),
            debug_sky: set("OMSI_DEBUG_SKY"),
            debug_draws: set("OMSI_DEBUG_DRAWS"),
            debug_cull: debug_cull_at.is_some(),
            debug_cull_at: debug_cull_at.and_then(|v| v.into_string().ok()),
            debug_shadow: debug_shadow_r.is_some(),
            debug_shadow_r: debug_shadow_r.and_then(|v| v.into_string().ok()),
            debug_flicker: set("OMSI_DEBUG_FLICKER"),
            only_surfaces: set("OMSI_ONLY_SURFACES"),
            skip_pipe: value("OMSI_SKIP_PIPE"),
            no_bundles: set("OMSI_NO_BUNDLES"),
            no_msaa_prepass: set("OMSI_NO_MSAA_PREPASS"),
            no_main_split: set("OMSI_NO_MAIN_SPLIT"),
            no_smoke: set("OMSI_NO_SMOKE"),
            no_snowfall: set("OMSI_NO_SNOWFALL"),
            no_coronas: set("OMSI_NO_CORONAS"),
            no_glare: set("OMSI_NO_GLARE"),
            debug_fog_lamps: set("OMSI_DEBUG_FOG_LAMPS"),
            no_rt_grade: set("OMSI_NO_RT_GRADE"),
        }
    }
}

/// What `render_inner` was asked to draw.
pub(crate) struct FrameArgs<'a> {
    pub target: &'a wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub camera: &'a Camera,
    pub lighting: &'a Lighting,
    pub with_overlays: bool,
    pub exclude_texture: Option<TextureId>,
    pub projection: Option<Mat4>,
    pub second_eye: bool,
}

/// What every phase of a frame shares: the view drawn, the frame's mode switches and its
/// targets (see `Renderer::begin_frame`).
pub(crate) struct FrameCtx<'a> {
    pub env: FrameEnv,
    pub target: &'a wgpu::TextureView,
    pub camera: &'a Camera,
    pub lighting: &'a Lighting,
    pub with_overlays: bool,
    pub exclude_texture: Option<TextureId>,
    pub projection: Option<Mat4>,
    pub second_eye: bool,
    /// the render origin
    pub ro: DVec3,
    /// the window's width (the HUD is drawn at the window's size)
    pub full_w: u32,
    /// the 3D picture's size (smaller than the window's with a render scale)
    pub width: u32,
    pub height: u32,
    pub vanilla_fxaa: bool,
    pub glass_on: bool,
    pub scaled: bool,
    pub scene_target: Option<(wgpu::TextureView, wgpu::BindGroup)>,
    pub aspect: f32,
    pub cam_rel: Vec3,
    pub xr_view: bool,
    pub lead_view: bool,
    pub enhanced_frame: bool,
    pub enhanced: bool,
    pub puddles_wanted: bool,
    pub reflection_frame: bool,
    pub masked_frame: bool,
    pub grid: [f32; 4],
    pub lamp_shadows: Vec<LampShadow>,
    pub rt_frame: bool,
    pub ao_on: bool,
    pub prepass_on: bool,
    pub dt: f32,
    pub overlays: Vec<(TextureId, [f32; 4])>,
}

impl FrameCtx<'_> {
    /// where the 3D picture is drawn: the scaled target, or the window's own
    pub(crate) fn scene_view(&self) -> &wgpu::TextureView {
        self.scene_target.as_ref().map(|t| &t.0).unwrap_or(self.target)
    }
}

/// OMSI_GPU_TIMERS: the query set of this frame (when it is timed) and the passes timed
/// so far.
pub(crate) struct PassTimers {
    pub set: Option<wgpu::QuerySet>,
    pub timed: Vec<&'static str>,
}

/// OMSI_PROFILE: time per stage, the mirrors apart from the window's picture
pub(crate) struct StageClock {
    t: std::time::Instant,
    with_overlays: bool,
}

impl StageClock {
    pub(crate) fn new(with_overlays: bool) -> StageClock {
        StageClock { t: std::time::Instant::now(), with_overlays }
    }

    pub(crate) fn stage(&mut self, r: &Renderer, window: &'static str, mirror: &'static str) {
        if r.profiling {
            let now = std::time::Instant::now();
            *r.stats
                .borrow_mut()
                .entry(if self.with_overlays { window } else { mirror })
                .or_default() += (now - self.t).as_secs_f64();
            self.t = now;
        }
    }
}

/// a material's alpha mode as the pipeline kind it is drawn with
pub(crate) fn kind_of(alpha: AlphaMode) -> u8 {
    match alpha {
        AlphaMode::Opaque => PIPE_OPAQUE,
        AlphaMode::Test => PIPE_ALPHA_TEST,
        AlphaMode::Blend => PIPE_BLEND,
    }
}

/// the three command encoders of a frame, finished side by side (see `finish_and_submit`):
/// the shadow maps, the depth prepass with the ambient occlusion, and the picture itself
pub(crate) struct Encoders {
    pub shadow: wgpu::CommandEncoder,
    pub prepass: wgpu::CommandEncoder,
    pub picture: wgpu::CommandEncoder,
}
