//! The voice chat of a LAN session, once a frame, and the bus radio key.

use super::*;

impl App {
    /// The voice chat (`voice`), once a frame of a session: started with it when the
    /// settings allow, the host's voice server asked for, the plugin told who is where.
    pub(super) fn tick_voice(&mut self, dt: f32) {
        let Some(lan) = self.lan.as_mut() else {
            self.voice = None;
            return;
        };
        // (a dedicated server has nobody to talk at its place; a joining game that lost
        // its host is in no session to talk in)
        if !self.settings.voice_chat || self.args.server.is_some() || !lan.connected {
            self.voice = None;
            return;
        }
        let v = self.voice.get_or_insert_with(|| crate::voice::Voice::new(crate::voice::DEFAULT_PORT));
        match lan.role {
            omsi_net::Role::Host => {
                // (once: hosted() reads voice.cfg)
                if !v.known() {
                    v.set_server(crate::voice::hosted());
                }
            }
            omsi_net::Role::Client => {
                if lan.connected && v.should_ask(dt) {
                    lan.command(1, "voice?");
                }
            }
        }
        let lan = self.lan.as_ref().unwrap();
        let my_bus = self.player.as_ref().map(|p| p.vehicle.position);
        let others = crate::voice::speakers(lan, &self.remotes, my_bus);
        let inside = if self.in_cab { Some(lan.my_id) } else { self.inside_remote };
        // driving a bus (not on foot): on the company radio automatically
        let on_radio = self.player.is_some() && !self.ego;
        let radio_keyed = on_radio && self.voice_radio_held();
        let listener = self.camera.as_ref().map(|c| crate::voice::Listener {
            at: c.position,
            yaw: c.yaw,
            inside,
            on_radio,
            radio_keyed,
        });
        let me = (lan.my_name.clone(), lan.my_id);
        if let Some(v) = self.voice.as_mut() {
            v.tick(dt, (&me.0, me.1), listener, &others);
        }
    }

    /// Is the bindable bus radio key (`voice_radio` in Controls) held right now?
    /// Keyboard chord or a controller button bound to the same action (held while down).
    pub(super) fn voice_radio_held(&self) -> bool {
        if self.pad_voice_radio {
            return true;
        }
        let held = |a: KeyCode, b: KeyCode| self.keys.contains(&a) || self.keys.contains(&b);
        let chord = omsi_content::input::chord(
            held(KeyCode::ShiftLeft, KeyCode::ShiftRight),
            held(KeyCode::ControlLeft, KeyCode::ControlRight),
            held(KeyCode::AltLeft, KeyCode::AltRight),
        );
        self.game_keys.iter().any(|b| {
            b.action.eq_ignore_ascii_case("voice_radio")
                && b.scan_code != 0
                && b.matches(chord)
                && self.keys.iter().any(|k| crate::keys::dik_code(*k) == Some(b.scan_code))
        })
    }
}
