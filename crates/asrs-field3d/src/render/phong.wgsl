// MeshPhongMaterial from three 0.168 (meshphong.glsl.js and its chunks), statement for statement,
// so the arithmetic order matches the GLSL that ANGLE compiles.

const MAX_HEMI: u32 = 4u;
const MAX_POINT: u32 = 8u;
const MAX_SPOT: u32 = 8u;
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

struct Frame {
    projection: mat4x4<f32>,
    // hemisphere, point, spot
    counts: vec4<u32>,
    hemisphere: array<HemisphereLight, MAX_HEMI>,
    point: array<PointLight, MAX_POINT>,
    spot: array<SpotLight, MAX_SPOT>,
};

struct Draw {
    model_view: mat4x4<f32>,
    normal_matrix: mat3x3<f32>,
    diffuse_opacity: vec4<f32>,
    emissive: vec4<f32>,
    specular_shininess: vec4<f32>,
    // vertex colors (0 none, 3 rgb, 4 rgba), FLIP_SIDED, DOUBLE_SIDED, OPAQUE
    flags: vec4<u32>,
    // USE_MAP, object id for the auxiliary pass
    extra: vec4<u32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
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
};

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
    out.clip = frame.projection * mv_position;
    // ANGLE's Metal translation of gl_Position: GL's [-w, w] depth range mapped to [0, w], and
    // y negated because WebGL renders bottom-up. Drawing upside down puts the multisample
    // pattern and the fill rule's tie-breaks on the same image pixels as upstream's; the
    // readback reverses the rows again.
    out.clip.z = (out.clip.z + out.clip.w) * 0.5;
    out.clip.y = -out.clip.y;
    out.view_position = -mv_position.xyz;
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

    for (var i = 0u; i < frame.counts.z; i++) {
        let light = frame.spot[i];
        let l_vector = light.position.xyz - geometry_position;
        let direction = fast_normalize(l_vector);
        let angle_cos = dot(direction, light.direction.xyz);
        let spot_attenuation = smoothstep(light.params.z, light.params.w, angle_cos);
        var color = vec3<f32>(0.0);
        if (spot_attenuation > 0.0) {
            let light_distance = length(l_vector);
            color = light.color.xyz * spot_attenuation;
            color *= distance_attenuation(light_distance, light.params.x, light.params.y);
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
