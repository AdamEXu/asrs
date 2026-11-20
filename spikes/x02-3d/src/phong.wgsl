struct Frame {
    proj: mat4x4<f32>,
    view: mat4x4<f32>,
    hemi_sky: vec4<f32>,
    hemi_ground: vec4<f32>,
    hemi_dir: vec4<f32>,
    point_pos: vec4<f32>,
    point_color: vec4<f32>,
};

struct Draw {
    model_view: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    color: vec4<f32>,
    specular: vec4<f32>,
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<uniform> draw: Draw;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

const RECIPROCAL_PI: f32 = 0.3183098861837907;

@vertex
fn vs(@location(0) pos: vec3<f32>, @location(1) nrm: vec3<f32>, @location(2) col: vec4<f32>) -> VsOut {
    var out: VsOut;
    let mv = draw.model_view * vec4<f32>(pos, 1.0);
    out.clip = frame.proj * mv;
    out.view_pos = -mv.xyz;
    var n = (draw.normal_matrix * vec4<f32>(nrm, 0.0)).xyz;
    if (draw.flags.y > 0.5) {
        n = -n;
    }
    out.normal = normalize(n);
    out.color = col.rgb;
    return out;
}

fn f_schlick(f0: vec3<f32>, f90: f32, dot_vh: f32) -> vec3<f32> {
    let fresnel = exp2((-5.55473 * dot_vh - 6.98316) * dot_vh);
    return f0 * (1.0 - fresnel) + (f90 * fresnel);
}

fn brdf_blinn_phong(l: vec3<f32>, v: vec3<f32>, n: vec3<f32>, spec: vec3<f32>, shininess: f32) -> vec3<f32> {
    let h = normalize(l + v);
    let dot_nh = saturate(dot(n, h));
    let dot_vh = saturate(dot(v, h));
    let f = f_schlick(spec, 1.0, dot_vh);
    let d = RECIPROCAL_PI * (shininess * 0.5 + 1.0) * pow(dot_nh, shininess);
    return f * (0.25 * d);
}

fn srgb_oetf(c: vec3<f32>) -> vec3<f32> {
    return select(pow(c, vec3<f32>(0.41666)) * 1.055 - vec3<f32>(0.055), c * 12.92, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var diffuse = draw.color.rgb;
    if (draw.flags.x > 0.5) {
        diffuse *= in.color;
    }
    var n = normalize(in.normal);
    if (draw.flags.z > 0.5 && !front) {
        n = -n;
    }
    let pos = -in.view_pos;
    let view_dir = normalize(in.view_pos);

    var direct_diffuse = vec3<f32>(0.0);
    var direct_specular = vec3<f32>(0.0);
    {
        let lv = frame.point_pos.xyz - pos;
        let dir = normalize(lv);
        let dist = length(lv);
        let atten = 1.0 / max(pow(dist, 2.0), 0.01);
        let light_color = frame.point_color.rgb * atten;
        let dot_nl = saturate(dot(n, dir));
        let irradiance = dot_nl * light_color;
        direct_diffuse += irradiance * RECIPROCAL_PI * diffuse;
        direct_specular += irradiance * brdf_blinn_phong(dir, view_dir, n, draw.specular.rgb, draw.specular.w);
    }
    let hemi_weight = 0.5 * dot(n, frame.hemi_dir.xyz) + 0.5;
    let irradiance = mix(frame.hemi_ground.rgb, frame.hemi_sky.rgb, hemi_weight);
    let indirect_diffuse = irradiance * RECIPROCAL_PI * diffuse;

    let outgoing = direct_diffuse + indirect_diffuse + direct_specular;
    return vec4<f32>(srgb_oetf(outgoing), draw.color.a);
}
