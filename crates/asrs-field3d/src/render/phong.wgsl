// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

// MeshPhongMaterial from three 0.168 (meshphong.glsl.js and its chunks), statement for statement,
// so the arithmetic order matches the GLSL that ANGLE compiles.

const MAX_HEMI: u32 = 4u;
const MAX_POINT: u32 = 8u;
const MAX_SPOT: u32 = 8u;
const MAX_SPOT_SHADOWS: u32 = 4u;
const RECIPROCAL_PI: f32 = 0.3183098861837907;

struct HemisphereLight {
    direction: vec4<f32>,
    sky_color: vec4<f32>,
    ground_color: vec4<f32>,
};

struct PointLight {
    position: vec4<f32>,
    color: vec4<f32>,
    // distance, decay
    params: vec4<f32>,
};

struct SpotLight {
    position: vec4<f32>,
    direction: vec4<f32>,
    color: vec4<f32>,
    // distance, decay, coneCos, penumbraCos
    params: vec4<f32>,
};

struct SpotShadow {
    matrix: mat4x4<f32>,
    // shadowIntensity, shadowBias, shadowNormalBias, shadowRadius
    params: vec4<f32>,
    // shadowMapSize
    map_size: vec4<f32>,
};

struct Frame {
    projection: mat4x4<f32>,
    // hemisphere, point, spot, always 0 (see opaque)
    counts: vec4<u32>,
    // ANGLE's runtime clip-space factors: y flip (-1) and depth sign (+1)
    clip_factors: vec4<f32>,
    hemisphere: array<HemisphereLight, MAX_HEMI>,
    point: array<PointLight, MAX_POINT>,
    spot: array<SpotLight, MAX_SPOT>,
    view: mat4x4<f32>,
    // spot light shadows, 0 unless the shadow map is enabled
    shadow_counts: vec4<u32>,
    spot_shadow: array<SpotShadow, MAX_SPOT_SHADOWS>,
};

struct Draw {
    model_view: mat4x4<f32>,
    normal_matrix: mat3x3<f32>,
    diffuse_opacity: vec4<f32>,
    emissive: vec4<f32>,
    specular_shininess: vec4<f32>,
    // vertex colors (0 none, 3 rgb, 4 rgba), FLIP_SIDED, DOUBLE_SIDED, OPAQUE
    flags: vec4<u32>,
    // USE_MAP, object id for the auxiliary pass, receiveShadow
    extra: vec4<u32>,
    model: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var shadow_maps: texture_2d_array<f32>;
@group(0) @binding(2) var shadow_sampler: sampler;
@group(1) @binding(0) var<uniform> draw: Draw;
@group(2) @binding(0) var map: texture_2d<f32>;
@group(2) @binding(1) var map_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

struct InstanceIn {
    @location(4) c0: vec4<f32>,
    @location(5) c1: vec4<f32>,
    @location(6) c2: vec4<f32>,
    @location(7) c3: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) spot_coord0: vec4<f32>,
    @location(5) spot_coord1: vec4<f32>,
    @location(6) spot_coord2: vec4<f32>,
    @location(7) spot_coord3: vec4<f32>,
};

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

fn finish_vertex(v: VertexIn, object_normal: vec3<f32>, mv_input: vec4<f32>) -> VertexOut {
    var out: VertexOut;
    out.uv = v.uv;
    if (draw.flags.x == 4u) {
        out.color = vec4<f32>(1.0);
        out.color *= v.color;
    } else if (draw.flags.x == 3u) {
        var c = vec3<f32>(1.0);
        c *= v.color.xyz;
        out.color = vec4<f32>(c, 1.0);
    } else {
        out.color = vec4<f32>(1.0);
    }
    var transformed_normal = draw.normal_matrix * object_normal;
    if (draw.flags.y != 0u) {
        transformed_normal = -transformed_normal;
    }
    out.normal = fast_normalize(transformed_normal);
    let mv_position = draw.model_view * mv_input;
    out.clip = angle_epilogue(frame.projection * mv_position);
    out.view_position = -mv_position.xyz;

    // worldpos_vertex and shadowmap_vertex
    let world_position = draw.model * mv_input;
    let shadow_world_normal = fast_normalize((vec4<f32>(transformed_normal, 0.0) * frame.view).xyz);
    var coords = array<vec4<f32>, MAX_SPOT_SHADOWS>();
    for (var i = 0u; i < frame.shadow_counts.x; i++) {
        let shadow_world_position = world_position + vec4<f32>(shadow_world_normal * frame.spot_shadow[i].params.z, 0.0);
        coords[i] = spot_light_coord(frame.spot_shadow[i].matrix, shadow_world_position);
    }
    out.spot_coord0 = coords[0];
    out.spot_coord1 = coords[1];
    out.spot_coord2 = coords[2];
    out.spot_coord3 = coords[3];
    return out;
}

@vertex
fn vs_mesh(v: VertexIn) -> VertexOut {
    return finish_vertex(v, v.normal, vec4<f32>(v.position, 1.0));
}

@vertex
fn vs_instanced(v: VertexIn, i: InstanceIn) -> VertexOut {
    let instance_matrix = mat4x4<f32>(i.c0, i.c1, i.c2, i.c3);
    let im = mat3x3<f32>(i.c0.xyz, i.c1.xyz, i.c2.xyz);
    var object_normal = v.normal;
    object_normal /= vec3<f32>(dot(im[0], im[0]), dot(im[1], im[1]), dot(im[2], im[2]));
    object_normal = im * object_normal;
    var mv_position = vec4<f32>(v.position, 1.0);
    mv_position = instance_matrix * mv_position;
    return finish_vertex(v, object_normal, mv_position);
}

// An opaque copy of v: frame.counts.w is always 0, and the compiler cannot see that.
fn opaque(v: vec4<f32>) -> vec4<f32> {
    return bitcast<vec4<f32>>(bitcast<vec4<u32>>(v) ^ vec4<u32>(frame.counts.w));
}

// spotLightMatrix[i] * shadowWorldPosition in the operation order Metal's fast-math compiler
// gives ANGLE's translation of shadowmap_vertex: translation column first, then y, x and z,
// each fused. The receiver's depth is compared against the shadow map's at full precision, so
// any other order turns 1-ulp differences into shadow acne on different pixels. The opaque
// steps stop the compiler from reordering the sum for this shader. The order was measured
// against ANGLE's Metal backend on Apple GPUs only: D3D11 and SwiftShader compile upstream's
// GLSL their own way, so T10 has to measure it again per backend.
fn spot_light_coord(m: mat4x4<f32>, v: vec4<f32>) -> vec4<f32> {
    var acc = opaque(m[3] * v.w);
    acc = opaque(fma(m[1], vec4<f32>(v.y), acc));
    acc = opaque(fma(m[0], vec4<f32>(v.x), acc));
    return fma(m[2], vec4<f32>(v.z), acc);
}

// metal::fast::normalize, which ANGLE emits for GLSL normalize.
fn fast_normalize(v: vec3<f32>) -> vec3<f32> {
    return v * inverseSqrt(dot(v, v));
}

fn f_schlick(f0: vec3<f32>, f90: f32, dot_vh: f32) -> vec3<f32> {
    let fresnel = exp2((-5.55473 * dot_vh - 6.98316) * dot_vh);
    return f0 * (1.0 - fresnel) + (f90 * fresnel);
}

fn d_blinn_phong(shininess: f32, dot_nh: f32) -> f32 {
    return RECIPROCAL_PI * (shininess * 0.5 + 1.0) * pow(dot_nh, shininess);
}

fn brdf_blinn_phong(light_dir: vec3<f32>, view_dir: vec3<f32>, normal: vec3<f32>, specular_color: vec3<f32>, shininess: f32) -> vec3<f32> {
    let half_dir = fast_normalize(light_dir + view_dir);
    let dot_nh = saturate(dot(normal, half_dir));
    let dot_vh = saturate(dot(view_dir, half_dir));
    let f = f_schlick(specular_color, 1.0, dot_vh);
    let g = 0.25;
    let d = d_blinn_phong(shininess, dot_nh);
    return f * (g * d);
}

fn brdf_lambert(diffuse_color: vec3<f32>) -> vec3<f32> {
    return RECIPROCAL_PI * diffuse_color;
}

fn distance_attenuation(light_distance: f32, cutoff_distance: f32, decay_exponent: f32) -> f32 {
    var distance_falloff = 1.0 / max(pow(light_distance, decay_exponent), 0.01);
    if (cutoff_distance > 0.0) {
        let x = light_distance / cutoff_distance;
        let x2 = x * x;
        let s = saturate(1.0 - x2 * x2);
        distance_falloff *= s * s;
    }
    return distance_falloff;
}

struct Reflected {
    direct_diffuse: vec3<f32>,
    direct_specular: vec3<f32>,
};

fn re_direct(light_direction: vec3<f32>, light_color: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, diffuse_color: vec3<f32>, r: ptr<function, Reflected>) {
    let dot_nl = saturate(dot(normal, light_direction));
    let irradiance = dot_nl * light_color;
    (*r).direct_diffuse += irradiance * brdf_lambert(diffuse_color);
    (*r).direct_specular += irradiance * brdf_blinn_phong(light_direction, view_dir, normal, draw.specular_shininess.xyz, draw.specular_shininess.w) * 1.0;
}

fn srgb_transfer_oetf(value: vec4<f32>) -> vec4<f32> {
    let low = vec3<f32>(value.rgb <= vec3<f32>(0.0031308));
    return vec4<f32>(mix(pow(value.rgb, vec3<f32>(0.41666)) * 1.055 - vec3<f32>(0.055), value.rgb * 12.92, low), value.a);
}

// ANGLE's fragment shaders always write [[sample_mask]] under MSAA. Writing it changes how the
// GPU shades pixels split between two triangles of one draw (without it, internal edges of
// meshes with non-linear vertex colours differ by up to 37 levels), so it is not removable.
struct FragmentOut {
    @location(0) color: vec4<f32>,
    @builtin(sample_mask) sample_mask: u32,
};

@fragment
fn fs(in: VertexOut, @builtin(front_facing) front_facing: bool) -> FragmentOut {
    var diffuse_color = draw.diffuse_opacity;
    var r: Reflected;
    r.direct_diffuse = vec3<f32>(0.0);
    r.direct_specular = vec3<f32>(0.0);
    let total_emissive_radiance = draw.emissive.xyz;
    let sampled = textureSample(map, map_sampler, in.uv);
    if (draw.extra.x != 0u) {
        diffuse_color *= sampled;
    }
    if (draw.flags.x == 4u) {
        diffuse_color *= in.color;
    } else if (draw.flags.x == 3u) {
        diffuse_color = vec4<f32>(diffuse_color.rgb * in.color.xyz, diffuse_color.a);
    }

    let face_direction = select(-1.0, 1.0, front_facing);
    var normal = fast_normalize(in.normal);
    if (draw.flags.z != 0u) {
        normal *= face_direction;
    }

    let material_diffuse = diffuse_color.rgb;
    let geometry_position = -in.view_position;
    let geometry_normal = normal;
    let geometry_view_dir = fast_normalize(in.view_position);

    for (var i = 0u; i < frame.counts.y; i++) {
        let light = frame.point[i];
        let l_vector = light.position.xyz - geometry_position;
        let direction = fast_normalize(l_vector);
        let light_distance = length(l_vector);
        var color = light.color.xyz;
        color *= distance_attenuation(light_distance, light.params.x, light.params.y);
        re_direct(direction, color, geometry_normal, geometry_view_dir, material_diffuse, &r);
    }

    let spot_coords = array<vec4<f32>, MAX_SPOT_SHADOWS>(in.spot_coord0, in.spot_coord1, in.spot_coord2, in.spot_coord3);
    for (var i = 0u; i < frame.counts.z; i++) {
        let light = frame.spot[i];
        let l_vector = light.position.xyz - geometry_position;
        let direction = fast_normalize(l_vector);
        let angle_cos = dot(direction, light.direction.xyz);
        let spot_attenuation = smoothstep(light.params.z, light.params.w, angle_cos);
        var color = vec3<f32>(0.0);
        var visible = false;
        if (spot_attenuation > 0.0) {
            let light_distance = length(l_vector);
            color = light.color.xyz * spot_attenuation;
            color *= distance_attenuation(light_distance, light.params.x, light.params.y);
            visible = any(color != vec3<f32>(0.0));
        }
        if (i < frame.shadow_counts.x && visible && draw.extra.z != 0u) {
            let shadow = frame.spot_shadow[i];
            color *= get_shadow(i, shadow.map_size.xy, shadow.params.x, shadow.params.y, shadow.params.w, spot_coords[i]);
        }
        re_direct(direction, color, geometry_normal, geometry_view_dir, material_diffuse, &r);
    }

    var irradiance = vec3<f32>(0.0);
    for (var i = 0u; i < frame.counts.x; i++) {
        let light = frame.hemisphere[i];
        let dot_nl = dot(geometry_normal, light.direction.xyz);
        let hemi_diffuse_weight = 0.5 * dot_nl + 0.5;
        irradiance += mix(light.ground_color.xyz, light.sky_color.xyz, hemi_diffuse_weight);
    }
    let indirect_diffuse = irradiance * brdf_lambert(material_diffuse);

    let outgoing_light = r.direct_diffuse + indirect_diffuse + r.direct_specular + vec3<f32>(0.0) + total_emissive_radiance;
    var alpha = diffuse_color.a;
    if (draw.flags.w != 0u) {
        alpha = 1.0;
    }
    var out: FragmentOut;
    out.color = srgb_transfer_oetf(vec4<f32>(outgoing_light, alpha));
    out.sample_mask = 0xffffffffu;
    return out;
}

struct AuxOut {
    @location(0) opaque_id: u32,
    @location(1) transparent_id: u32,
};

// Object id and facing, so a fold of one mesh over itself (its silhouette against its own far
// side) counts as an edge too.
@fragment
fn fs_aux(@builtin(front_facing) front_facing: bool) -> AuxOut {
    let id = draw.extra.y * 2u + select(0u, 1u, front_facing);
    var out: AuxOut;
    out.opaque_id = id;
    out.transparent_id = id;
    return out;
}
