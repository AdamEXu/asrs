@group(0) @binding(0) var opaque_ids: texture_multisampled_2d<f32>;
@group(0) @binding(1) var transparent_ids: texture_multisampled_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) u32 {
    let at = vec2<i32>(position.xy);
    let opaque = textureLoad(opaque_ids, at, 0);
    let transparent = textureLoad(transparent_ids, at, 0);
    var mixed = false;
    for (var i = 1u; i < textureNumSamples(opaque_ids); i++) {
        mixed |= any(textureLoad(opaque_ids, at, i) != opaque);
        mixed |= any(textureLoad(transparent_ids, at, i) != transparent);
    }
    return select(0u, 1u, mixed);
}
