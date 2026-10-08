//! Where the player looks from: what the population may not be seen doing.

use super::*;

/// Where the player looks from, for putting cars on the road and taking them off only
/// where nobody sees it happen.
#[derive(Debug, Clone, Copy)]
pub struct Viewer {
    pub pos: DVec3,
    pub forward: DVec3,
    /// Tangents of half the horizontal and vertical field of view.
    pub tan_x: f64,
    pub tan_y: f64,
    /// Beyond this distance nothing shows (fog, or a car smaller than a pixel) (m).
    pub range: f64,
    /// What the renderer leaves out (see `omsi_render::RenderOptions`): objects smaller on
    /// the screen than `min_size` (the original's measure), and farther than `max_dist`
    /// (0 = no limit); `fov` is the vertical field of view (radians).
    pub min_size: f64,
    pub max_dist: f64,
    pub fov: f64,
}

/// A car further away than this is below two pixels on a 900-line screen (m).
pub const VISIBLE_RANGE: f64 = 900.0;

/// Within this distance of the camera no car appears or vanishes, seen or not: the mirrors
/// and a turn of the head see what is near (see [`Traffic::may_appear`]).
pub const NEVER_VANISH_WITHIN: f64 = 150.0;
/// Within this distance a vehicle appears or vanishes only behind something, wherever the
/// player looks (see `Traffic::hidden`).
pub const NEAR_HIDE: f64 = 350.0;

/// Within this distance of the camera an AI vehicle is animated and drawn even out of the
/// view (m): the mirrors look behind, and a car beside the view throws its shadow into it.
pub const UNSEEN_NEAR: f64 = 80.0;

impl Viewer {
    /// The view of a camera at `position` looking along `forward`, with a vertical field of
    /// view of `fov_deg` and its far plane at `far`, a picture `aspect` wide to high, in fog
    /// that hides everything beyond `fog_range`.
    pub fn from_camera(position: DVec3, forward: DVec3, fov_deg: f32, far: f32, aspect: f64, fog_range: f64) -> Viewer {
        let tan_y = (fov_deg as f64 * 0.5).to_radians().tan();
        Viewer {
            pos: position,
            forward: forward.normalize_or_zero(),
            tan_x: tan_y * aspect.max(0.2),
            tan_y,
            range: fog_range.min(VISIBLE_RANGE).min(far as f64),
            min_size: 0.0,
            max_dist: 0.0,
            fov: (fov_deg as f64).to_radians(),
        }
    }

    /// A wider picture than the camera's own (a triple screen's side panels): the tangents
    /// of its half-angles, horizontal and vertical. The size limit stays the camera's.
    pub fn with_extent(mut self, extent: Option<(f64, f64)>) -> Viewer {
        if let Some((tan_x, tan_y)) = extent {
            self.tan_x = self.tan_x.max(tan_x);
            self.tan_y = self.tan_y.max(tan_y);
        }
        self
    }

    /// The renderer's culling as well (`RenderOptions::min_obj_size`, `max_obj_dist`).
    pub fn with_culling(mut self, min_size: f32, max_dist: f32) -> Viewer {
        self.min_size = min_size.max(0.0) as f64;
        self.max_dist = max_dist.max(0.0) as f64;
        self
    }

    /// Would the renderer draw an object of radius `r` this far away at all? Beyond that a
    /// car can come and go in plain view without anybody seeing it happen.
    pub fn draws(&self, dist: f64, r: f64) -> bool {
        // (the renderer measures a vehicle by a sphere about its origin, which may stand
        // well off its middle: half as much again, and a metre, to be sure)
        let r = r * 1.5 + 1.0;
        if self.max_dist > 0.0 && dist > self.max_dist + r {
            return false;
        }
        self.min_size <= 0.0 || 2.0 * r / (dist.max(0.01) * self.fov.max(1e-3)) >= self.min_size
    }

    /// Does a sphere of radius `r` at `p` lie within the view frustum and range?
    pub fn frames(&self, p: DVec3, r: f64) -> bool {
        let rel = p - self.pos;
        let dist = rel.length();
        if dist <= r {
            return true;
        }
        if dist - r > self.range {
            return false;
        }
        let f = self.forward;
        let mut right = f.cross(DVec3::Z);
        if right.length() < 1e-3 {
            right = DVec3::X;
        }
        let right = right.normalize();
        let up = right.cross(f);
        let z = rel.dot(f);
        if z < -r {
            return false;
        }
        let (x, y) = (rel.dot(right), rel.dot(up));
        x.abs() <= z * self.tan_x + r * (1.0 + self.tan_x * self.tan_x).sqrt()
            && y.abs() <= z * self.tan_y + r * (1.0 + self.tan_y * self.tan_y).sqrt()
    }
}
