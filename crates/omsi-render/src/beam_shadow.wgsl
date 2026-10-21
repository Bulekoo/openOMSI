// The shadows in a lamp's beam (the fog pass: its camera uniform has the lamp shadow tiles and
// the far shadow map's texture, see `fog_lamps.wgsl`). The other shaders that share
// `lamp_air.wgsl` take `beam_shadow_none.wgsl`'s: no map to look up, the beam goes on.

// Which of the four lamp shadow tiles (under the far shadow map, see `lamp_shadow_at` in the
// scene's lamps) belongs to lamp `li`: 0..3, or 4 for a lamp with none. The player's
// headlamps - marked in `pos.w` - share the tile marked -2.
fn beam_tile(li: u32, l: PointLight) -> u32 {
    let own = max(-l.pos.w, 0.0) > 15.5;
    var k = 4u;
    for (var j = 0u; j < 4u; j = j + 1u) {
        if ((camera.lamp_shadow[j] >= 0.0 && u32(camera.lamp_shadow[j]) == li) || (own && camera.lamp_shadow[j] < -1.5)) {
            k = j;
        }
    }
    return k;
}

// How much of a beam reaches a point of the air, in tile `k` of its lamp's map: 0 where a
// pole, a car or a wall stands between the lamp and the point, so that the fog shows the
// shadow in its beam as it shows the sun's in a shaft. A tap of the hardware's filtered
// compare; off the tile, or without a map, the beam goes on.
fn beam_reaches(k: u32, x: vec3<f32>) -> f32 {
    if (k > 3u) {
        return 1.0;
    }
    let q = camera.lamp_view_proj[k] * vec4<f32>(x, 1.0);
    if (q.w <= 0.0) {
        return 1.0;
    }
    let ndc = q.xyz / q.w;
    if (abs(ndc.x) >= 1.0 || abs(ndc.y) >= 1.0 || ndc.z >= 1.0 || ndc.z <= 0.0) {
        return 1.0;
    }
    let dims = vec2<f32>(textureDimensions(t_shadow_far));
    let tile = dims.x * 0.25;
    let tuv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    let tx = clamp(tuv * tile, vec2<f32>(1.5), vec2<f32>(tile - 1.5));
    let at = (vec2<f32>(f32(k) * tile, dims.x) + tx) / dims;
    // (a little more off the occluder than a surface takes: the air has no surface to
    // acne, and the lamp's own housing must not shade its beam)
    return textureSampleCompareLevel(t_shadow_far, s_shadow, at, ndc.z - 0.0002);
}
