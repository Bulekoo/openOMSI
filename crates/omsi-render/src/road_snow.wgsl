// Snow on the carriageways: how far it has built up, the ruts the traffic keeps in it and
// the fresh tracks the tyres press into it (see `Renderer::set_snow_track_rect`).
//
// The field is a square of `place.z` metres that wraps around the world (a torus): texel
// (i, j) holds the world's point (x, y) where floor(x / texel) and floor(y / texel) are i and
// j modulo its size, so the app only rewrites the tiles the camera moves into. It is kept
// tile after tile (`TRACK_TILE` texels a side), each row after row, a texel an RGBA8 word:
//   r: the ruts of the lanes' wheel tracks (0..1)
//   g: a tyre has pressed the snow here (1)
//   b, a: how much snow had fallen when it did (16 bits, see `state.y`): the track fills
//         as more falls on it

struct SnowTrack {
    // xy: the render origin modulo the field's side (m), z: the side (m), w: 1 while the
    // field holds the area around the camera
    place: vec4<f32>,
    // x: how far the snow covers the roads (0..1; below 0 the weather's "snow on road"
    // decides, as before), y: the snow fallen so far (thousandths of a full cover, modulo
    // 65536), z: how deep the lanes' ruts show (0..1), w: unused
    state: vec4<f32>,
};

@group(0) @binding(20) var<storage, read> snow_track_px: array<u32>;
@group(0) @binding(21) var<uniform> snow_track: SnowTrack;

const TRACK_TEXELS: i32 = 2048;
const TRACK_TILE: i32 = 256;

// One texel's rut and fresh track (the track fading as the snow fallen since fills it).
fn snow_track_texel(p: vec2<i32>) -> vec2<f32> {
    let q = ((p % TRACK_TEXELS) + TRACK_TEXELS) % TRACK_TEXELS;
    let tile = (q.y / TRACK_TILE) * (TRACK_TEXELS / TRACK_TILE) + q.x / TRACK_TILE;
    let i = (tile * TRACK_TILE + q.y % TRACK_TILE) * TRACK_TILE + q.x % TRACK_TILE;
    let t = unpack4x8unorm(snow_track_px[i]);
    let then = round(t.b * 255.0) * 256.0 + round(t.a * 255.0);
    let since = (snow_track.state.y - then + 65536.0) % 65536.0;
    // (some 15 % of a cover fallen on it and a track begins to blur, 40 % and it is gone)
    let fresh = t.g * (1.0 - smoothstep(150.0, 400.0, since));
    return vec2<f32>(t.r, fresh);
}

// The rut (x) and the fresh track (y) at a point of the render frame, blended between the
// four texels around it (each decoded first: the fill time does not blend).
fn snow_track_at(world: vec2<f32>) -> vec2<f32> {
    if (snow_track.place.w < 0.5) {
        return vec2<f32>(0.0);
    }
    let q = (world + snow_track.place.xy) * (f32(TRACK_TEXELS) / snow_track.place.z) - 0.5;
    let b = floor(q);
    let f = q - b;
    let i = vec2<i32>(b);
    let a00 = snow_track_texel(i);
    let a10 = snow_track_texel(i + vec2<i32>(1, 0));
    let a01 = snow_track_texel(i + vec2<i32>(0, 1));
    let a11 = snow_track_texel(i + vec2<i32>(1, 1));
    return mix(mix(a00, a10, f.x), mix(a01, a11, f.x), f.y);
}

// Whether the roads' snow is the built-up kind (`state.x` at or above 0) rather than the
// weather's on/off "snow on road".
fn road_snow_dynamic() -> bool {
    return snow_track.state.x >= 0.0;
}

// The snow on a carriageway at `world` (render frame): x the cover (0..1), y the slush in
// the ruts (0..1: the wheels keep it wet and grey), z a fresh tyre track (0..1).
fn road_snow(world: vec3<f32>) -> vec3<f32> {
    let base = clamp(snow_track.state.x, 0.0, 1.0);
    if (base <= 0.0) {
        return vec3<f32>(0.0);
    }
    // the map's own coordinates (modulo the pattern's period): the patches stay put
    let m = world.xy + camera.world_origin.xy;
    // It does not settle evenly: first a dusting in patches a stride or two across and in
    // the grain of the asphalt, which close up as it goes on falling.
    // (three octaves, the finer ones turned against the coarse: a single lattice's cells
    // showed as square blots where the cover's edge ran through them)
    let m2 = vec2<f32>(m.x * 0.8 - m.y * 0.6, m.x * 0.6 + m.y * 0.8);
    let m3 = vec2<f32>(m.x * 0.28 + m.y * 0.96, m.y * 0.28 - m.x * 0.96);
    let patches = vnoise_f(m, 0.45, vec2<f32>(3.1, 7.7)) * 0.55 + vnoise_f(m2, 1.3, vec2<f32>(11.3, 2.9)) * 0.3 + vnoise_f(m3, 3.7, vec2<f32>(5.9, 13.1)) * 0.15;
    var cover = smoothstep(patches - 0.35, patches + 0.1, base * 1.35 - 0.15);
    let tracks = snow_track_at(world.xy);
    // The ruts: the wheels of the traffic keep two strips of each lane down to slush, the
    // more of it the more snow there is to churn; with a mere dusting they show the asphalt.
    let rut = tracks.x * snow_track.state.z;
    let slush = rut * smoothstep(0.15, 0.7, base);
    cover = cover * (1.0 - rut * 0.85);
    // A fresh track: the tread pressed the snow flat and grey. Its edge sharpened from the
    // texels' blend (a soft ramp over a whole texel read as a blur), and the pressed snow
    // grainy - the tread's blocks, as fine as the eye can still tell apart.
    let edge = smoothstep(0.2, 0.75, tracks.y);
    let grain = 0.75 + 0.25 * vnoise_f(m2, 14.0, vec2<f32>(1.7, 9.3));
    let track = edge * grain;
    cover = cover * (1.0 - track * 0.6);
    return vec3<f32>(cover, max(slush, track * 0.5 * base), track);
}
