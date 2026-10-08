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
