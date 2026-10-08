//! The groups of `App`'s state, one per part of the game (see `App`).

use super::*;

/// The game's sound: the audio engine and what plays through it.
pub(crate) struct SoundState {
    /// The player's bus radio as internet radio.
    pub(crate) radio: radio::Radio,
    pub(crate) audio: Option<omsi_audio::AudioEngine>,
    /// Sounds of the world around the camera (rain, footsteps).
    pub(crate) ambience: Option<ambience::Ambience>,
    /// Positional voice through GreenTeaSpeak in a session (`voice`).
    pub(crate) voice: Option<crate::voice::Voice>,
}

/// The VR headset: its session (Windows), the navigator shown in it, the cockpit pointer and the picture zoom.
pub(crate) struct VrState {
    #[cfg(windows)]
    pub(crate) vr: Option<crate::openxr::Vr>,
    pub(crate) vr_nav_profiles: crate::vr_navigator::Profiles,
    pub(crate) vr_nav_edit: Option<crate::vr_navigator::Editing>,
    /// Last Windows mouse position used for the unbounded VR cockpit pointer.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) vr_cursor_physical: Option<(f32, f32)>,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) vr_cursor_warp_pending: Option<(f32, f32)>,
    /// Right mouse button toggles the headset picture zoom.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) vr_zoom_active: bool,
}

/// The multiplayer session: the LAN or server connection and the other players seen in it.
pub(crate) struct NetState {
    /// Other players on foot whose avatars are drawn (their ids).
    pub(crate) remote_walkers: Vec<u32>,
    /// The player on foot is in this other player's bus (see `lan`: drawn from inside).
    pub(crate) inside_remote: Option<u32>,
    /// A dedicated server said we administer it (`admin`).
    pub(crate) is_admin: bool,
    /// LAN session, and the other players' buses (drawn and heard like AI vehicles) with the
    /// chat line.
    pub(crate) lan: Option<omsi_net::LanSession>,
    pub(crate) remotes: lan::LanGame,
}

/// What the game talks to besides itself: the OMSI and Lua plugins, Discord, Steam, the website's "playing now" and the look for a newer release.
pub(crate) struct Integrations {
    /// Keys pressed (true) and let go since the Lua plugins' last frame.
    pub(crate) plugin_keys: Vec<(String, bool)>,
    /// What happened since the Lua plugins' last frame: crashes, people knocked down,
    /// stops skipped (see `plugins::queue_event`).
    pub(crate) plugin_events: Vec<omsi_plugin::GameEvent>,
    /// The Lua plugins' panels and notifications on the screen (`omsi.ui`).
    pub(crate) plugin_panels: crate::plugin_ui::PluginPanels,
    /// Discord's "Playing openOMSI" status, and when it was last brought up to date.
    pub(crate) discord: Option<crate::discord::Discord>,
    pub(crate) discord_t: f32,
    // Steamworks API layer and it's last updated time
    #[cfg(steam)]
    pub(crate) steam: Option<crate::steam::Steam>,
    /// The look for a newer release during the session (cards over the navigator).
    pub(crate) update_watch: crate::update_watch::UpdateWatch,
    /// "Playing now" on the website (None: not counted, setting `presence`).
    pub(crate) presence: Option<crate::presence::Presence>,
    /// The OMSI plugins (`plugins/*.opl`), loaded with the first frame.
    pub(crate) plugins: Option<omsi_plugin::Plugins>,
}

/// How the frames go and what is measured or scripted about them: the frame rate, the profile, the stutters, the frame-rate governor, the `OMSI_INPUT` script, screenshots and the log.
pub(crate) struct PerfState {
    pub(crate) fps: f32,
    /// Per-stage frame time accumulators (OMSI_PROFILE), seconds.
    pub(crate) profile: std::collections::BTreeMap<&'static str, f64>,
    /// `profile` as it was at the start of the last frame: what a slow frame spent where.
    pub(crate) profile_prev: std::collections::BTreeMap<&'static str, f64>,
    pub(crate) total_frames: u32,
    /// `OMSI_INPUT` script: (seconds after start, command), in order.
    pub(crate) input_script: Vec<(f32, String)>,
    /// A pending screenshot: its output path and whether touch controls are composited over it.
    /// Scripted `shot <file>` captures keep the controls for visual tests; player screenshots
    /// leave them out so the camera button produces a clean image.
    pub(crate) shot: Option<(PathBuf, bool)>,
    pub(crate) frames: u32,
    pub(crate) fps_t: Instant,
    /// What the log has said (see applog.rs).
    pub(crate) log_state: crate::applog::LogState,
    /// Frames longer than 50 ms (stutters) and the worst frame, for the exit summary.
    pub(crate) spikes: u32,
    pub(crate) worst_ms: f32,
    /// The frame-rate governor's two-second window.
    /// Window seconds, frames, and time waiting on presentation/GPU in that window.
    pub(crate) governor: (f32, u32, f32),
    /// Readings in a row at the smallest render scale still waiting for the card.
    pub(crate) governor_low: u32,
    /// Cumulative presentation wait at the previous frame, independent of OMSI_PROFILE.
    pub(crate) governor_wait_prev: f64,
    /// OMSI_PROFILE: process CPU seconds, time and frame count once the start-up is over,
    /// for the CPU time a frame costs (the wall time says little on a busy machine).
    pub(crate) cpu_mark: Option<(f64, Instant, u32)>,
}

/// The drawing around the renderer: the wgpu instance and surface, the tile streaming, the bus mirrors, the window's visibility and what stands in for it.
pub(crate) struct GfxState {
    pub(crate) instance: wgpu::Instance,
    pub(crate) surface: Option<SurfaceState<'static>>,
    /// Tile streaming around the camera (the window's default).
    pub(crate) streamer: Option<tiles::Streamer>,
    /// The window spans the triple screen's three monitors: fullscreen would shrink it to one.
    pub(crate) spanned: bool,
    /// Mirror pictures due (see `MIRROR_RATE`), and which mirror is next.
    pub(crate) mirror_budget: f32,
    pub(crate) mirrors_seen: usize,
    pub(crate) mirror_turn: usize,
    /// With no real-time reflections: the bus whose mirrors are frozen (see
    /// `MIRROR_FREEZE_REDRAW`).
    pub(crate) frozen_mirrors: Option<FrozenMirrors>,
    /// The mirror panels laid over the picture (see `mirror_hud`).
    pub(crate) mirror_hud: crate::mirror_hud::MirrorHud,
    /// The window is minimised or out of sight, as its events last said.
    pub(crate) window_hidden: bool,
    /// OMSI 2's route arrows over the road (the `nav_arrows` setting).
    pub(crate) route_arrows: crate::route_arrows::RouteArrows,
    /// Frames the window was hidden for (they are not drawn) and whether the exit is under way.
    pub(crate) hidden_frames: u32,
    /// Stand-in for the window's frame while the window is hidden (OMSI_RENDER_OCCLUDED).
    pub(crate) stand_in: Option<wgpu::Texture>,
}

/// The camera's state besides the camera itself: the head turned and zoomed per view, the switch between cameras, the outside camera's distance, the free camera's speed and the pedestrian view.
pub(crate) struct ViewState {
    /// The map is open but the first area is still loading: the view to start with.
    pub(crate) starting: Option<Camera>,
    pub(crate) speed: f32,
    /// The idle head sway waiting where it is while the cursor is on a control
    /// (see `head_idle::Hold`).
    pub(crate) head_idle_hold: crate::head_idle::Hold,
    /// OMSI's pedestrian ("ego") view: the free camera walking at eye height on whatever
    /// people stand on (`view_set_ego`, F11).
    pub(crate) ego: bool,
    /// The camera is in the own bus's cab this frame (see RedrawRequested).
    pub(crate) in_cab: bool,
    /// How far the player has turned the head (driver, passenger) or swung the outside
    /// camera around the bus, and how far that camera sits from it.
    pub(crate) look: (f32, f32),
    /// Where the view is drawn between that angle and the one of the frame before: the way
    /// the mouse (or the stick, or the keys) went is eased in, so the head glides to the
    /// angle asked for rather than jumping to it (`look_smoothing_ms`; 0 keeps it equal to
    /// `look`). Only the camera reads this - everything that turns the view writes `look`.
    pub(crate) look_smooth: (f32, f32),
    /// Each view keeps its own `look` (as OMSI's cameras do): turning the outside camera
    /// (F3) leaves the driver's head (F1) where it was. `look_view` is the view `look`
    /// belongs to now; see `App::sync_view_look`.
    pub(crate) view_looks: std::collections::HashMap<String, (f32, f32)>,
    pub(crate) look_view: String,
    /// Smooth switch between two cockpit cameras (arrow keys), see `CamBlend`.
    pub(crate) cam_blend: CamBlend,
    /// The zoom of the views inside the bus (driver, passenger): their field of view is
    /// the camera's times this (the mouse wheel, + and -, a pinch), per view.
    pub(crate) view_zoom: std::collections::HashMap<String, f32>,
    /// Eased Space return in flight (F1 only): ((look from), (zoom from), seconds in,
    /// look key it started from). A hand on the view cancels it; other views reset
    /// instantly. If the camera changes mid-glide, the originating camera is
    /// finalized straight ahead instead of keeping a partial angle.
    pub(crate) f1_reset: Option<((f32, f32), f32, f32, String)>,
    pub(crate) orbit: f32,
}

/// What the player's hands and head do: the keys and buttons held, the cursor, mouse and controller driving, head tracking, the phone's touch controls and the switch being dragged.
pub(crate) struct InputState {
    pub(crate) cursor: (f32, f32),
    pub(crate) window_focused: bool,
    /// The window lost the focus or was minimised or hidden: the keyboard and the mouse
    /// work nothing until it has the focus again (`App::input_lost` / `input_back`).
    pub(crate) input_away: bool,
    pub(crate) keys: hashbrown::HashSet<KeyCode>,
    /// Door trigger groups currently held by the Shift+number shortcut. Keeping the
    /// release until physical key-up prevents latched button states and door chatter.
    pub(crate) door_key_triggers: hashbrown::HashMap<KeyCode, Vec<String>>,
    pub(crate) mouse_look: bool,
    /// The left and right mouse buttons held.
    pub(crate) buttons_held: (bool, bool),
    /// The middle button held (looks round; the right button zooms).
    pub(crate) mmb_held: bool,
    /// The right button (or both) held: OMSI's mouse zoom (0x82c5f8) - moving the mouse up
    /// widens the view in the bus or takes the outside camera further away, by the value at
    /// the press over 500 pixels: (the cursor's height then, the zoom or distance then).
    pub(crate) both_drag: Option<(f32, f32)>,
    /// Seconds Ctrl+Shift+Page Up/Down has been held (the clock runs faster the longer).
    pub(crate) clock_hold: f32,
    /// A controller button held for looking left, right, up, down (`view_look_*`).
    pub(crate) pad_look: [bool; 4],
    /// A controller button held for the multiplayer bus radio (`voice_radio`).
    pub(crate) pad_voice_radio: bool,
    /// The arrow keys turned the head (a glance that comes back when they are let go).
    pub(crate) arrow_glance: bool,
    /// Head tracking (Settings → head tracking), started with the first frame that wants it.
    pub(crate) headtrack: Option<crate::headtrack::HeadTracker>,
    /// When head tracking last failed to start (tried again a few seconds later).
    pub(crate) headtrack_failed: Option<std::time::Instant>,
    /// Last TrackIR/OpenTrack output scales, used to keep the displayed camera position
    /// fixed while a sensitivity slider is changed.
    pub(crate) headtrack_scale_last: Option<[f32; 6]>,
    /// Per-axis compensation for a live sensitivity change.
    pub(crate) headtrack_scale_bias: [f32; 6],
    /// Last inversion state; inversion is a direction change, not a new camera origin.
    pub(crate) headtrack_invert_last: Option<[bool; 6]>,
    /// Steering wheels, pedals, joysticks and gamepads (`Inputs/gamectrler.cfg`).
    pub(crate) controllers: Option<crate::controllers::Controllers>,
    /// OMSI's mouse control (`toggel_mouse_ctrl`, O): the cursor's place steers (across) and
    /// works the pedals (up throttle, down brake).
    pub(crate) mouse_drive: bool,
    /// Mouse steering: the steering it gives (fraction of the full lock) and how long (s)
    /// it still eases in after being switched on (OMSI: a second, see app_events).
    pub(crate) mouse_steer: (f32, f32),
    /// Mouse steering past the window's edge: the lock the mouse added while the cursor stood
    /// pinned at the left or right edge (-1..1 of full lock). OMSI divides the width by the
    /// speed, and at 30 km/h the edge of the screen was a third of the lock, with nowhere
    /// further to move.
    pub(crate) mouse_edge: f32,
    /// Where the cursor steered when the right button began to look round: it goes back
    /// there when the button is let go, so the wheel does not jump to where looking left it.
    pub(crate) steer_cursor: Option<(f32, f32)>,
    /// The cursor is put in the middle of the window before the mouse steers for the first
    /// time (a game started with the mouse steering on: wherever the cursor was, the wheel
    /// turned and the bus drove off on full throttle).
    pub(crate) center_cursor: bool,
    /// The cursor hidden while a controller drives: where it stood.
    pub(crate) cursor_hidden: Option<(f32, f32)>,
    /// The wheel's place when it last counted as moved.
    pub(crate) last_ctl_steer: Option<f32>,
    /// The mouse's throttle and brake (eased in with the steering).
    pub(crate) mouse_pedals: (f32, f32),
    /// The speed mouse steering divides by, smoothed.
    pub(crate) mouse_kmh: f32,
    /// The speed a gamepad stick's steering divides by, smoothed (as `mouse_kmh`).
    pub(crate) pad_kmh: f32,
    /// Where a gamepad stick turns the wheel to, smoothed (`pad_steer_smooth`).
    pub(crate) pad_steer_target: f32,
    /// OMSI's global key actions from `Inputs/keyboard.cfg` ([game]).
    pub(crate) game_keys: Vec<omsi_content::KeyBinding>,
    /// Keys (DirectInput scan codes, no modifier) the player bound on the Controls page to
    /// something the original's keyboard.cfg does not have there: a driving preset (W A S D,
    /// the arrows) leaves them alone - D bound to the gearbox is the gearbox, not "steer right".
    pub(crate) own_keys: std::collections::HashSet<i32>,
    /// The same for keys held with Shift (a Shift+number of the player's own is not a door key).
    pub(crate) own_shift: std::collections::HashSet<i32>,
    /// The left button is held on a switch: mouse movement turns it.
    pub(crate) dragging: bool,
    /// The left button is held on a page of the bus (an `[htmltexture]`): its script texture
    /// index and the place on it the pointer was last seen.
    pub(crate) html_pressed: Option<(usize, f32, f32)>,
    /// The same for a page of a scenery object: its map id, script texture index and place.
    pub(crate) html_object_pressed: Option<(i64, usize, f32, f32)>,
    /// Cursor movement (logical pixels) while dragging a switch, not yet handed to the
    /// script: `<event>_drag` fires once a frame with it (see `Player::drag`).
    pub(crate) drag_delta: (f32, f32),
    /// The mouse cursor currently shows the hand (it is over a switch).
    pub(crate) cursor_kind: u8,
    /// The on-screen controls of a phone (see `touch.rs`).
    pub(crate) touch: crate::touch::Touch,
}
