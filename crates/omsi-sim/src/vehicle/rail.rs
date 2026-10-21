//! A rail vehicle's speed: the scripts' drive and brakes, the track's grade and curve and the
//! weather on the rails go through [`crate::rail::dynamics`] instead of through the road
//! tyres of the rigid body, which know nothing of rails.

use super::*;
use crate::rail::{self, Conditions, Consist, Forces, RailClass};

/// What the track under a rail vehicle is like, told by whoever lays it on the track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RailEnv {
    /// Rise per metre in the vehicle's forward direction.
    pub grade: f32,
    /// Radius of the curve under it (m), infinite on the straight.
    pub radius: f32,
    pub in_tunnel: bool,
}

impl VehicleInstance {
    /// The track the vehicle runs on, or `None` for a road vehicle: its speed then comes from
    /// the rail dynamics, not from the road tyres.
    pub fn set_rail_env(&mut self, env: Option<RailEnv>) {
        self.rail_env = env;
    }

    /// The whole consist as the dynamics see it: this vehicle and everything coupled to it.
    pub fn rail_consist(&self, class: RailClass) -> Consist {
        let mass_of = |d: &omsi_vehicle::Vehicle| if d.mass < 100.0 { d.mass * 1000.0 } else { d.mass }.max(500.0);
        let parts = std::iter::once(&self.ty.def).chain(self.trailers.iter().map(|t| &t.ty.def));
        let (mut mass, mut length, mut wheels, mut driven) = (0.0f32, 0.0f32, 0usize, 0usize);
        // (the weight on driven axles: each car's mass shared over its axles)
        let mut driven_weight = 0.0f32;
        for d in parts {
            let m = mass_of(d);
            mass += m;
            length += d.bounding_box.map(|b| b[1]).unwrap_or(12.0);
            let n = d.axles.len().max(1);
            let dn = d.axles.iter().filter(|a| a.driven).count();
            wheels += n;
            driven += dn;
            driven_weight += m * dn as f32 / n as f32;
        }
        let _ = (wheels, driven);
        let aero = crate::rigid::realism::Aero::of(self.ty.def.bounding_box.map(|b| [b[0], b[1], b[2]]), mass_of(&self.ty.def));
        Consist {
            class,
            mass,
            driven_share: if mass > 0.0 { driven_weight / mass } else { 1.0 },
            cars: 1 + self.trailers.len(),
            cd_a: aero.cd_a,
            length,
        }
    }

    /// The drive and the brakes the scripts ask for this frame, N: `m_wheel` is the torque at
    /// the driven wheels, `brakes` the per-wheel brake forces of the vehicle's own axles and
    /// the coupled cars' come from theirs. `None` for a vehicle whose scripts give neither
    /// (the stock trains are scripted for the AI only).
    fn rail_script_forces(&self, m_wheel: f32, brakes: &[f32]) -> Option<Forces> {
        let own: f32 = brakes.iter().sum();
        let towed: f32 = self.trailers.iter().flat_map(|t| t.v_brakes.iter()).map(|&id| self.get(id).max(0.0)).sum();
        let brake = own + towed;
        if m_wheel.abs() < 1.0 && brake < 1.0 {
            return None;
        }
        let driven = self.ty.def.axles.iter().filter(|a| a.driven).map(|a| a.wheel_diameter / 2.0).collect::<Vec<_>>();
        let radius = if driven.is_empty() { 0.45 } else { driven.iter().sum::<f32>() / driven.len() as f32 }.max(0.15);
        Some(Forces { drive: m_wheel / radius, brake, track_brake: 0.0 })
    }

    /// The speed along the rails after `dt` as the rail dynamics make it (`v0` the speed
    /// before): `None` for a vehicle that is not on a track.
    fn rail_solve(&mut self, dt: f32, v0: f32, m_wheel: f32, brakes: &[f32]) -> Option<rail::Outcome> {
        let (env, class) = (self.rail_env?, RailClass::of(&self.ty.def)?);
        let consist = self.rail_consist(class);
        let cond = Conditions {
            grade: env.grade,
            radius: env.radius,
            wetness: self.host.street_cond,
            temperature: self.host.temperature,
            in_tunnel: env.in_tunnel,
            air_density: crate::rigid::realism::air_density(self.host.temperature, 0.0),
            headwind: 0.0,
            sanding: false,
        };
        let from_scripts = self.rail_script_forces(m_wheel, brakes);
        self.rail_scripted |= from_scripts.is_some();
        let c = self.physics.controls;
        let forces = if self.rail_scripted {
            from_scripts.unwrap_or_default()
        } else {
            let reverse = self.var("rail_reverse").is_some_and(|r| r > 0.5);
            rail::dynamics::pedal_forces(class, consist.mass, c.throttle, c.brake, v0, reverse)
        };
        let out = rail::step(&consist, &cond, &forces, v0, dt);
        if omsi_cfg::flags::OMSI_DEBUG_RAIL.is_set() {
            self.rail_log += dt;
            if self.rail_log >= 0.5 {
                self.rail_log = 0.0;
                log::info!(
                    "rail: {} v {:.1} km/h a {:.2} m/s² | M_Wheel {:.0} Nm -> drive {:.0} N, brake {:.0} N ({}) | mass {:.0} kg, driven {:.2}, {} cars, adhesion {:.2} | spin {} slide {} resist {:.0} N",
                    self.ty.def.path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                    out.speed * 3.6, out.accel, m_wheel, forces.drive, forces.brake,
                    if self.rail_scripted { "scripts" } else { "pedals" },
                    consist.mass, consist.driven_share, consist.cars,
                    rail::dynamics::adhesion(out.speed, cond.wetness, cond.temperature, cond.sanding),
                    out.spinning, out.sliding, out.resistance
                );
            }
        }
        self.rail_out = Some(out);
        Some(out)
    }

    /// After the rigid body's step: the rail dynamics' speed. The wheels turn with it - a
    /// rail wheel does not slip as a road tyre does - and the body keeps to the line.
    pub(super) fn rail_override(&mut self, rb: &mut crate::rigid::RigidBody, dt: f32, v0: f32, m_wheel: f32, brakes: &[f32]) {
        let Some(out) = self.rail_solve(dt, v0, m_wheel, brakes) else { return };
        let fwd = rb.orientation.mul_vec3(glam::Vec3::Y);
        rb.velocity = fwd * out.speed;
        rb.omega = glam::Vec3::ZERO;
        for w in rb.wheels.iter_mut() {
            w.spin = out.speed / w.radius.max(0.05);
            w.slipping = out.spinning && w.driven;
            w.locked = false;
            w.rpm = w.spin * 60.0 / std::f32::consts::TAU;
            w.rotation_deg = (w.rotation_deg + w.spin.to_degrees() * dt).rem_euclid(360.0);
        }
        rb.accel_body = glam::Vec3::new(0.0, out.accel, 9.81);
    }

    /// The same for the simple model, which a rail vehicle runs on (it is put on its track,
    /// not on the road): returns the distance covered this step.
    pub(super) fn rail_override_simple(&mut self, dt: f32, v0: f32, m_wheel: f32, brakes: &[f32]) -> Option<f32> {
        let out = self.rail_solve(dt, v0, m_wheel, brakes)?;
        self.physics.speed = out.speed;
        self.physics.accel = glam::Vec3::new(0.0, out.accel, 0.0);
        for w in self.physics.wheels.iter_mut().flatten() {
            let omega = out.speed / w.radius.max(0.05);
            w.rpm = omega * 60.0 / std::f32::consts::TAU;
            w.rotation_deg = (w.rotation_deg + omega.to_degrees() * dt).rem_euclid(360.0);
        }
        Some(out.speed * dt)
    }

    /// What the rail dynamics made of the last step (spinning or sliding wheels, resistance).
    pub fn rail_outcome(&self) -> Option<rail::Outcome> {
        self.rail_out
    }
}
