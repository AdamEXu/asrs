// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

// PCFSoftShadowMap: MeshDepthMaterial with RGBADepthPacking (depth.glsl.js, packing.glsl.js)
// for the shadow pass, and getShadow from shadowmap_pars_fragment.glsl.js for receivers.

const UNPACK_FACTORS4: vec4<f32> = vec4<f32>(255.0 / 256.0, 255.0 / 256.0 / 256.0, 255.0 / 256.0 / 65536.0, 1.0 / 16777216.0);
const PACK_UPSCALE: f32 = 256.0 / 255.0;
const SHIFT_RIGHT8: f32 = 1.0 / 256.0;
const INV255: f32 = 1.0 / 255.0;

fn texture2d_compare(layer: u32, uv: vec2<f32>, compare: f32) -> f32 {
    return step(compare, dot(textureSampleLevel(shadow_maps, shadow_sampler, uv, layer, 0.0), UNPACK_FACTORS4));
}

fn get_shadow(layer: u32, shadow_map_size: vec2<f32>, shadow_intensity: f32, shadow_bias: f32, shadow_radius: f32, coord: vec4<f32>) -> f32 {
    var shadow = 1.0;
    var shadow_coord = coord;
    // Metal compiles these two lines to fma(z, 1 / w, bias), as it does upstream's. An exact
    // divide or an unfused add, through opaque(), adds acne.
    shadow_coord = vec4<f32>(shadow_coord.xyz / shadow_coord.w, shadow_coord.w);
    shadow_coord.z += shadow_bias;
    let in_frustum = shadow_coord.x >= 0.0 && shadow_coord.x <= 1.0 && shadow_coord.y >= 0.0 && shadow_coord.y <= 1.0;
    let frustum_test = in_frustum && shadow_coord.z <= 1.0;
    if (frustum_test) {
        let texel_size = vec2<f32>(1.0) / shadow_map_size;
        let dx = texel_size.x;
        let dy = texel_size.y;
        var uv = shadow_coord.xy;
        let f = fract(uv * shadow_map_size + 0.5);
        uv -= f * texel_size;
        let z = shadow_coord.z;
        shadow = (
            texture2d_compare(layer, uv, z) +
            texture2d_compare(layer, uv + vec2<f32>(dx, 0.0), z) +
            texture2d_compare(layer, uv + vec2<f32>(0.0, dy), z) +
            texture2d_compare(layer, uv + texel_size, z) +
            mix(texture2d_compare(layer, uv + vec2<f32>(-dx, 0.0), z),
                texture2d_compare(layer, uv + vec2<f32>(2.0 * dx, 0.0), z),
                f.x) +
            mix(texture2d_compare(layer, uv + vec2<f32>(-dx, dy), z),
                texture2d_compare(layer, uv + vec2<f32>(2.0 * dx, dy), z),
                f.x) +
            mix(texture2d_compare(layer, uv + vec2<f32>(0.0, -dy), z),
                texture2d_compare(layer, uv + vec2<f32>(0.0, 2.0 * dy), z),
                f.y) +
            mix(texture2d_compare(layer, uv + vec2<f32>(dx, -dy), z),
                texture2d_compare(layer, uv + vec2<f32>(dx, 2.0 * dy), z),
                f.y) +
            mix(mix(texture2d_compare(layer, uv + vec2<f32>(-dx, -dy), z),
                    texture2d_compare(layer, uv + vec2<f32>(2.0 * dx, -dy), z),
                    f.x),
                mix(texture2d_compare(layer, uv + vec2<f32>(-dx, 2.0 * dy), z),
                    texture2d_compare(layer, uv + vec2<f32>(2.0 * dx, 2.0 * dy), z),
                    f.x),
                f.y)
        ) * (1.0 / 9.0);
    }
    return mix(1.0, shadow, shadow_intensity);
}

fn pack_depth_to_rgba(v: f32) -> vec4<f32> {
    if (v <= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    if (v >= 1.0) {
        return vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }
    let a = modf(v * 16777216.0);
    let b = modf(a.whole * SHIFT_RIGHT8);
    let g = modf(b.whole * SHIFT_RIGHT8);
    return vec4<f32>(g.whole * INV255, g.fract * PACK_UPSCALE, b.fract * PACK_UPSCALE, a.fract);
}

struct DepthOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) high_precision_zw: vec2<f32>,
};

fn finish_depth(mv_input: vec4<f32>) -> DepthOut {
    var out: DepthOut;
    let mv_position = draw.model_view * mv_input;
    let clip = frame.projection * mv_position;
    out.high_precision_zw = clip.zw;
    out.clip = angle_epilogue(clip);
    return out;
}

@vertex
fn vs_depth(v: VertexIn) -> DepthOut {
    return finish_depth(vec4<f32>(v.position, 1.0));
}

@vertex
fn vs_depth_instanced(v: VertexIn, i: InstanceIn) -> DepthOut {
    let instance_matrix = mat4x4<f32>(i.c0, i.c1, i.c2, i.c3);
    var mv_position = vec4<f32>(v.position, 1.0);
    mv_position = instance_matrix * mv_position;
    return finish_depth(mv_position);
}

@fragment
fn fs_depth(in: DepthOut) -> @location(0) vec4<f32> {
    let frag_coord_z = 0.5 * in.high_precision_zw[0] / in.high_precision_zw[1] + 0.5;
    return pack_depth_to_rgba(frag_coord_z);
}
