// What ANGLE's D3D11 backend makes of upstream's GLSL: HLSL's mul, normalize and rsq in GLSL's
// order.

// GL's [-w, w] depth mapped to [0, w], and y flipped as in metal.wgsl. The factor comes from a
// uniform, as in ANGLE, so the compiler cannot fold it into the matrix product.
fn angle_epilogue(clip: vec4<f32>) -> vec4<f32> {
    var out = clip;
    out.z = (out.z + out.w) * 0.5;
    out.y = out.y * frame.clip_factors.x;
    return out;
}

fn spot_light_coord(light: u32, m: mat4x4<f32>, v: vec4<f32>) -> vec4<f32> {
    return m * v;
}

fn gl_normalize(v: vec3<f32>) -> vec3<f32> {
    return normalize(v);
}

fn gl_normalize2(v: vec2<f32>) -> vec2<f32> {
    return normalize(v);
}
