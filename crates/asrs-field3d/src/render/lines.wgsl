// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

// LineMaterial from examples/jsm/lines with worldUnits, dashes and alpha to coverage off, and
// LineBasicMaterial (meshbasic.glsl.js) for Line, LineSegments and GridHelper. For both, Draw's
// diffuse_opacity is the colour and opacity; for fat lines specular_shininess holds linewidth
// and the viewport resolution.

struct FatLineIn {
    @location(0) position: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(8) instance_start: vec3<f32>,
    @location(9) instance_end: vec3<f32>,
};

struct FatLineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

fn fast_normalize2(v: vec2<f32>) -> vec2<f32> {
    return v * inverseSqrt(dot(v, v));
}

fn trim_segment(start: vec4<f32>, end: vec4<f32>) -> vec4<f32> {
    let a = frame.projection[2][2];
    let b = frame.projection[3][2];
    let near_estimate = -0.5 * b / a;
    let alpha = (near_estimate - start.z) / (end.z - start.z);
    return vec4<f32>(mix(start.xyz, end.xyz, alpha), end.w);
}

@vertex
fn vs_fat_line(v: FatLineIn) -> FatLineOut {
    var out: FatLineOut;
    let linewidth = draw.specular_shininess.x;
    let resolution = draw.specular_shininess.yz;
    let aspect = resolution.x / resolution.y;
    var start = draw.model_view * vec4<f32>(v.instance_start, 1.0);
    var end = draw.model_view * vec4<f32>(v.instance_end, 1.0);
    out.uv = v.uv;
    let perspective = frame.projection[2][3] == -1.0;
    if (perspective) {
        if (start.z < 0.0 && end.z >= 0.0) {
            end = trim_segment(start, end);
        } else if (end.z < 0.0 && start.z >= 0.0) {
            start = trim_segment(end, start);
        }
    }
    let clip_start = frame.projection * start;
    let clip_end = frame.projection * end;
    let ndc_start = clip_start.xyz / clip_start.w;
    let ndc_end = clip_end.xyz / clip_end.w;
    var dir = ndc_end.xy - ndc_start.xy;
    dir.x *= aspect;
    dir = fast_normalize2(dir);
    var offset = vec2<f32>(dir.y, -dir.x);
    dir.x /= aspect;
    offset.x /= aspect;
    if (v.position.x < 0.0) {
        offset *= -1.0;
    }
    if (v.position.y < 0.0) {
        offset += -dir;
    } else if (v.position.y > 1.0) {
        offset += dir;
    }
    offset *= linewidth;
    offset /= resolution.y;
    var clip = clip_end;
    if (v.position.y < 0.5) {
        clip = clip_start;
    }
    offset *= clip.w;
    clip = vec4<f32>(clip.xy + offset, clip.zw);
    out.clip = angle_epilogue(clip);
    return out;
}

fn round_cap_discard(uv: vec2<f32>) {
    if (abs(uv.y) > 1.0) {
        let a = uv.x;
        let b = select(uv.y + 1.0, uv.y - 1.0, uv.y > 0.0);
        let len2 = a * a + b * b;
        if (len2 > 1.0) {
            discard;
        }
    }
}

@fragment
fn fs_fat_line(in: FatLineOut) -> FragmentOut {
    round_cap_discard(in.uv);
    let alpha = draw.diffuse_opacity.w;
    let diffuse_color = vec4<f32>(draw.diffuse_opacity.xyz, alpha);
    var out: FragmentOut;
    out.color = srgb_transfer_oetf(vec4<f32>(diffuse_color.rgb, alpha));
    out.sample_mask = 0xffffffffu;
    return out;
}

@fragment
fn fs_fat_line_aux(in: FatLineOut) -> AuxOut {
    round_cap_discard(in.uv);
    var out: AuxOut;
    out.opaque_id = draw.extra.y * 2u;
    out.transparent_id = draw.extra.y * 2u;
    return out;
}

struct BasicOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_basic(v: VertexIn) -> BasicOut {
    var out: BasicOut;
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
    let mv_position = draw.model_view * vec4<f32>(v.position, 1.0);
    out.clip = angle_epilogue(frame.projection * mv_position);
    return out;
}

@fragment
fn fs_basic(in: BasicOut) -> FragmentOut {
    var diffuse_color = draw.diffuse_opacity;
    if (draw.flags.x == 4u) {
        diffuse_color *= in.color;
    } else if (draw.flags.x == 3u) {
        diffuse_color = vec4<f32>(diffuse_color.rgb * in.color.xyz, diffuse_color.a);
    }
    var indirect_diffuse = vec3<f32>(0.0);
    indirect_diffuse += vec3<f32>(1.0);
    indirect_diffuse *= diffuse_color.rgb;
    let outgoing_light = indirect_diffuse;
    var alpha = diffuse_color.a;
    if (draw.flags.w != 0u) {
        alpha = 1.0;
    }
    var out: FragmentOut;
    out.color = srgb_transfer_oetf(vec4<f32>(outgoing_light, alpha));
    out.sample_mask = 0xffffffffu;
    return out;
}

@fragment
fn fs_basic_aux() -> AuxOut {
    var out: AuxOut;
    out.opaque_id = draw.extra.y * 2u;
    out.transparent_id = draw.extra.y * 2u;
    return out;
}
