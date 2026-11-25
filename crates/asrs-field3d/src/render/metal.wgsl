// What ANGLE's Metal backend makes of upstream's GLSL.

// ANGLE's Metal epilogue for gl_Position, with its factors read from a uniform as ANGLE does so
// the compiler cannot fold them into the matrix product: depth sign, GL's [-w, w] depth mapped
// to [0, w], and y flipped because WebGL renders bottom-up into its drawing buffer and render
// targets alike. Drawing upside down puts the multisample pattern and the fill rule's
// tie-breaks on the same image pixels as upstream's; readback and texture sampling both see
// GL's row order.
fn angle_epilogue(clip: vec4<f32>) -> vec4<f32> {
    var out = clip;
    out.z = out.z * frame.clip_factors.y;
    out.z = (out.z + out.w) * 0.5;
    out.y = out.y * frame.clip_factors.x;
    return out;
}

// An opaque copy of v: frame.counts.w is always 0, and the compiler cannot see that.
fn opaque(v: vec4<f32>) -> vec4<f32> {
    return bitcast<vec4<f32>>(bitcast<vec4<u32>>(v) ^ vec4<u32>(frame.counts.w));
}

// spotLightMatrix[i] * shadowWorldPosition in the operation order Metal's fast-math compiler
// gives ANGLE's translation of shadowmap_vertex, which reassociates each light's product its own
// way: w, y, x, z for light 0, y, x, z, w for light 1 and x, y, z, w for light 2, each fused. The
// receiver's depth is compared against the shadow map's at full precision, so any other order
// turns 1-ulp differences into shadow acne on different pixels. The opaque steps stop the
// compiler from reordering the sum for this shader. The orders were measured for upstream's three
// spot lights against ANGLE's Metal backend on Apple GPUs only.
fn spot_light_coord(light: u32, m: mat4x4<f32>, v: vec4<f32>) -> vec4<f32> {
    if (light == 0u) {
        var acc = opaque(m[3] * v.w);
        acc = opaque(fma(m[1], vec4<f32>(v.y), acc));
        acc = opaque(fma(m[0], vec4<f32>(v.x), acc));
        return fma(m[2], vec4<f32>(v.z), acc);
    }
    if (light == 1u) {
        var acc = opaque(m[1] * v.y);
        acc = opaque(fma(m[0], vec4<f32>(v.x), acc));
        acc = opaque(fma(m[2], vec4<f32>(v.z), acc));
        return fma(m[3], vec4<f32>(v.w), acc);
    }
    var acc = opaque(m[0] * v.x);
    acc = opaque(fma(m[1], vec4<f32>(v.y), acc));
    acc = opaque(fma(m[2], vec4<f32>(v.z), acc));
    return fma(m[3], vec4<f32>(v.w), acc);
}

// metal::fast::normalize, which ANGLE emits for GLSL normalize.
fn gl_normalize(v: vec3<f32>) -> vec3<f32> {
    return v * inverseSqrt(dot(v, v));
}

fn gl_normalize2(v: vec2<f32>) -> vec2<f32> {
    return v * inverseSqrt(dot(v, v));
}
