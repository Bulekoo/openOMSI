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
