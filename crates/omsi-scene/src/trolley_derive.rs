//! The overhead wire of a map, read off the map itself. Trolleybus maps made for OMSI 2 draw
//! the wire as splines whose type has two thin strips high over its centre line (a few
//! centimetres wide, some 5 m up, half a metre apart): the strips are the wires, the spline's
//! centre line is where the pair runs, and the hangers and bends are scenery objects with no
//! behaviour. This finds those splines, takes their centre lines at the wires' height and
//! hands them to `omsi_trolley` as a wire network; three of them meeting end to end are a frog.
//! A `trolley.txt` beside the map overrides it (see `docs/TROLLEYBUS.md`).

use glam::DVec2;
use omsi_trolley::WireNet;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What a spline type's profile says of it: `Some((centre, spacing, height))` for a pair of
/// wires (lateral centre of the pair, distance between them, height over the spline).
pub fn wire_profile(def: &omsi_scenery::sli::Spline) -> Option<(f64, f64, f64)> {
    let mut strips: Vec<(f64, f64)> = Vec::new();
    for p in &def.profiles {
        let pts = &p.points;
        if pts.len() < 2 {
            continue;
        }
        let (x0, x1) = pts.iter().fold((f32::MAX, f32::MIN), |a, q| (a.0.min(q.x), a.1.max(q.x)));
        let zmin = pts.iter().fold(f32::MAX, |a, q| a.min(q.z));
        let zmax = pts.iter().fold(f32::MIN, |a, q| a.max(q.z));
        // (thin, high over the road, and thin in height too: a wire, not a mast or a fence)
        if x1 - x0 <= 0.2 && zmin >= 3.0 && zmax - zmin <= 0.25 {
            strips.push((((x0 + x1) * 0.5) as f64, ((zmin + zmax) * 0.5) as f64));
        }
    }
    if def.profiles.len() > 3 || strips.len() != 2 {
        return None;
    }
    let spacing = (strips[0].0 - strips[1].0).abs();
    if !(0.2..=1.2).contains(&spacing) {
        return None;
    }
    Some(((strips[0].0 + strips[1].0) * 0.5, spacing, (strips[0].1 + strips[1].1) * 0.5))
}

/// The wire network of a map's tiles, or `None` where there is none.
pub fn derive(root: &Path, tiles: &[(i32, i32, PathBuf)], chrono_dirs: &[PathBuf]) -> Option<WireNet> {
    let text = derive_text(root, tiles, chrono_dirs)?;
    let mut net = WireNet::parse(&text);
    net.drawn_by_map = true;
    (!net.is_empty()).then_some(net)
}

/// The same as the `trolley.txt` text that says it.
pub fn derive_text(root: &Path, tiles: &[(i32, i32, PathBuf)], chrono_dirs: &[PathBuf]) -> Option<String> {
    let ts = omsi_map::tile_size();
    let mut slis: HashMap<String, Option<(f64, f64, f64, omsi_scenery::sli::Spline)>> = HashMap::new();
    let mut spans: Vec<(f64, Vec<[f64; 3]>)> = Vec::new();
    for (tx, ty, path) in tiles {
        let Some(tile) = omsi_world::map_index::read_tile(path, chrono_dirs) else { continue };
        let o2 = DVec2::new(*tx as f64 * ts, *ty as f64 * ts);
        for sp in tile.splines.iter().filter(|s| !s.deleted && !s.file.trim().is_empty() && s.length >= 0.5) {
            let key = sp.file.trim().replace('\\', "/").to_ascii_lowercase();
            let entry = slis.entry(key).or_insert_with(|| {
                let def = omsi_scenery::sli::Spline::load(&omsi_cfg::resolve_path(root, &sp.file)).ok()?;
                let (c, s, h) = wire_profile(&def)?;
                Some((c, s, h, def))
            });
            let Some((centre, spacing, height, def)) = entry.as_ref() else { continue };
            let curve = omsi_geometry::SplineCurve::from_map(sp, o2).with_sli(def);
            let side = if sp.mirror { -1.0 } else { 1.0 };
            let turn_deg = if curve.radius != 0.0 { (curve.length / curve.radius).abs().to_degrees() } else { 0.0 };
            let n = ((curve.length / 3.0).ceil() as usize).max((turn_deg / 1.5).ceil() as usize).clamp(1, 600);
            let pts: Vec<[f64; 3]> = (0..=n)
                .map(|k| {
                    let (p, _) = omsi_geometry::skewed_point(&curve, curve.length * k as f64 / n as f64, side * centre, *height);
                    [p.x, p.y, p.z]
                })
                .collect();
            spans.push((*spacing, pts));
        }
    }
    if spans.is_empty() {
        return None;
    }
    let mut out = String::from("[trolley_net]\nvoltage 600\nspacing 0.52\n\n[trolley_section]\nid 1\nname map\nlive 1\nregen_kw 0\nfeeder_ohm 0.04\n\n");
    // (`OMSI_TROLLEY_DEAD_AT=x,y,r;...`: the wire within r metres of a point is a dead section)
    let zones: Vec<[f64; 3]> = omsi_cfg::flags::OMSI_TROLLEY_DEAD_AT
        .var()
        .map(|v| v.split(';').filter_map(|z| {
            let n: Vec<f64> = z.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (n.len() == 3).then(|| [n[0], n[1], n[2]])
        }).collect())
        .unwrap_or_default();
    if !zones.is_empty() {
        out.push_str("[trolley_section]\nid 2\nname dead\nlive 0\n\n");
    }
    let dead = |p: &[f64; 3]| zones.iter().any(|z| (p[0] - z[0]).hypot(p[1] - z[1]) <= z[2]);
    let seg_dead = |a: &[f64; 3], b: &[f64; 3]| dead(&[(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, 0.0]);
    for (spacing, pts) in &spans {
        // runs of segments on one side of a zone's edge, each a span of its own (the next one
        // starts where it ends: a dead section is joined to a live one by an insulator)
        let mut start = 0;
        for k in 1..pts.len() {
            let last = k + 1 == pts.len();
            let change = !last && seg_dead(&pts[k - 1], &pts[k]) != seg_dead(&pts[k], &pts[k + 1]);
            if last || change {
                let d = !zones.is_empty() && seg_dead(&pts[start], &pts[start + 1]);
                out.push_str(&format!("[trolley_span]\nsection {}\nspacing {spacing:.3}\n", if d { 2 } else { 1 }));
                for p in &pts[start..=k] {
                    out.push_str(&format!("p {:.3} {:.3} {:.3}\n", p[0], p[1], p[2]));
                }
                out.push('\n');
                start = k;
            }
        }
    }
    // frogs: three span ends within 30 cm of each other
    let mut ends: Vec<[f64; 3]> = Vec::new();
    for (_, pts) in &spans {
        ends.push(pts[0]);
        ends.push(*pts.last().unwrap());
    }
    let mut used = vec![false; ends.len()];
    let mut n_frogs = 0;
    // (the ends in a grid of 30 cm cells: a search through all of them for each end is
    // quadratic in a map's thousands)
    let cell = |p: &[f64; 3]| ((p[0] / 0.3).floor() as i64, (p[1] / 0.3).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (j, e) in ends.iter().enumerate() {
        grid.entry(cell(e)).or_default().push(j);
    }
    for i in 0..ends.len() {
        if used[i] {
            continue;
        }
        let (cx, cy) = cell(&ends[i]);
        let near: Vec<usize> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dy| (cx + dx, cy + dy)))
            .filter_map(|c| grid.get(&c))
            .flatten()
            .copied()
            .filter(|j| (ends[*j][0] - ends[i][0]).hypot(ends[*j][1] - ends[i][1]) < 0.3)
            .collect();
        for j in &near {
            used[*j] = true;
        }
        if near.len() == 3 {
            let p = ends[i];
            out.push_str(&format!("[trolley_switch]\nname frog {n_frogs}\npos {:.3} {:.3} {:.3}\nmode power\ntrigger 9\nsense 12\n\n", p[0], p[1], p[2]));
            log::info!("trolley frog {n_frogs} at ({:.1}, {:.1})", p[0], p[1]);
            n_frogs += 1;
        }
    }
    log::info!("trolley wire derived from the map's splines: {} spans, {} frogs", spans.len(), n_frogs);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(strips: &[(f32, f32)]) -> omsi_scenery::sli::Spline {
        let mut s = omsi_scenery::sli::Spline::default();
        for (x, z) in strips {
            s.profiles.push(omsi_scenery::sli::SplineProfile {
                texture: 0,
                points: vec![
                    omsi_scenery::sli::SplineProfilePoint { x: x - 0.01, z: *z, u: 0.0, v_scale: 0.2 },
                    omsi_scenery::sli::SplineProfilePoint { x: x + 0.01, z: *z + 0.02, u: 0.0, v_scale: 0.2 },
                ],
            });
        }
        s
    }

    #[test]
    fn two_thin_strips_high_up_are_a_pair_of_wires() {
        let (c, s, h) = wire_profile(&profile(&[(-0.26, 5.04), (0.26, 5.04)])).unwrap();
        assert!(c.abs() < 1e-6 && (s - 0.52).abs() < 1e-6 && (h - 5.05).abs() < 0.02);
        // one strip is a tram's wire, strips on the ground are road paint, a wide strip a roof
        assert!(wire_profile(&profile(&[(0.0, 5.04)])).is_none());
        assert!(wire_profile(&profile(&[(-0.26, 0.0), (0.26, 0.0)])).is_none());
        assert!(wire_profile(&profile(&[(-1.0, 5.0), (1.0, 5.0)])).is_none());
    }
}
