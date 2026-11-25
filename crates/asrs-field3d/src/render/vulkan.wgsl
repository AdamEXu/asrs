// What ANGLE's Vulkan backend makes of upstream's GLSL: SPIR-V operation for operation, in GLSL's
// order. GL_CLIP_SPACE is true where the device has GL's clip space (gl_clip.rs).

fn angle_epilogue(clip: vec4<f32>) -> vec4<f32> {
    if (GL_CLIP_SPACE) {
        return clip;
    }
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
