//! Hardware bans as an administration does them - the same whoever administers: a dedicated
//! server's admin from the game or the web, or the host of a game hosting by code. The lists
//! themselves are `bans`; here is what a ban does to the session and what comes back.

use crate::bans::{self, Scope};
use omsi_net::LanSession;

/// What a player sent away by a ban reads.
fn why_text(reason: &str, until: Option<u64>) -> String {
    let mut s = String::from("you are banned from this session");
    if let Some(u) = until {
        let left = u.saturating_sub(omsi_admin::unix_now());
        s.push_str(&format!(" for {}", span(left)));
    }
    if !reason.is_empty() {
        s.push_str(": ");
        s.push_str(reason);
    }
    s
}

/// `3 hours`, `2 days`: a time left, roughly.
pub fn span(secs: u64) -> String {
    match secs {
        0..=5399 => format!("{} min", secs.div_ceil(60).max(1)),
        5400..=172_799 => format!("{} h", secs.div_ceil(3600)),
        _ => format!("{} days", secs.div_ceil(86_400)),
    }
}

/// Ban the machine of player `id` and send the player away. `by` is the admin's label for the
/// record. The message is what the admin is told; an Err is a ban that was not made (the
/// player has not been touched).
pub fn hardware_ban(lan: &mut LanSession, id: u32, scope: Scope, hours: Option<u32>, reason: &str, by: &str) -> Result<String, String> {
    let Some(hw) = lan.peer_hardware_id(id).map(str::to_string) else {
        return Err("that player's game sent no hardware identifier (an older game, or it switched it off): use 'ban' to keep them out of this run by name and address".into());
    };
    if lan.hardware_id.as_deref() == Some(hw.as_str()) {
        return Err("that is this machine itself (the host's own): it cannot be banned".into());
    }
    let name = lan.peers().find(|p| p.pose.id == id).map(|p| p.pose.name.clone()).unwrap_or_default();
    let now = omsi_admin::unix_now();
    let until = hours.map(|h| now + h as u64 * 3600);
    let ban = {
        let Some(mut store) = bans::global() else { return Err("this session keeps no ban list".into()) };
        store.add(scope, &hw, &name, reason, by, until, now)?
    };
    lan.kick(id, &why_text(&ban.reason, until), false);
    let mut seen = 0;
    bans::sync(lan, &mut seen);
    Ok(format!("{} banned ({}{}), ban {}", if name.is_empty() { format!("player {id}") } else { name }, if scope == Scope::Owner { "all sessions" } else { "this session" }, until.map(|u| format!(", for {}", span(u - now))).unwrap_or_default(), ban.id))
}

/// Lift ban `id` of either list.
pub fn unban(lan: &mut LanSession, id: &str) -> Result<String, String> {
    let ban = {
        let Some(mut store) = bans::global() else { return Err("this session keeps no ban list".into()) };
        store.remove(id)?
    };
    let mut seen = 0;
    bans::sync(lan, &mut seen);
    Ok(format!("ban {} lifted ({})", ban.id, if ban.name.is_empty() { "no name".to_string() } else { ban.name }))
}

/// The bans that hold, for the admins' panels.
pub fn views() -> Vec<omsi_admin::BanView> {
    let Some(store) = bans::global() else { return Vec::new() };
    store
        .active(omsi_admin::unix_now())
        .into_iter()
        .map(|b| omsi_admin::BanView { id: b.id, name: b.name, reason: b.reason, by: b.by, at: b.at, until: b.until, scope: b.scope.name().to_string() })
        .collect()
}
