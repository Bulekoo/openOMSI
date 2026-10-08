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
