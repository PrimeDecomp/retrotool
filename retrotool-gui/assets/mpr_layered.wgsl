// Reconstructed from MPR PhysicallyBased hash 0b8c1a78, NVN source 66:
// layered resource 07d578a9-47c0-4a4d-aa69-573473fb8d2c, permutation022C7D2B63205B00;
// single-map resource 5beb2ff2-107f-4b46-8138-4ff85676505a, permutation022C4D2B6B2C5A00.
// The caller checks the exact shader profile and positive layer smoothness.
// These equations do not describe every material or emulate Maxwell FP32 bits.

fn mpr_inverse_transpose3(matrix: mat3x3<f32>) -> mat3x3<f32> {
    // CTransform4f::GetInverseTranspose, 0x71000706ac. Its fallback is
    // identity in view space, and its cutoff is strict abs(det) > 2^-27.
    let c0 = cross(matrix[1], matrix[2]);
    let c1 = cross(matrix[2], matrix[0]);
    let c2 = cross(matrix[0], matrix[1]);
    let det = dot(matrix[0], c0);
    if (abs(det) > 0.000000007450580596923828125) {
        return mat3x3<f32>(c0, c1, c2) * (1.0 / det);
    }
    return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
}

fn mpr_vertex_frame_to_world(model: mat4x4<f32>, world_to_view: mat4x4<f32>, view_to_world: mat4x4<f32>) -> mat3x3<f32> {
    let model_view = world_to_view * model;
    let normal_view = mpr_inverse_transpose3(mat3x3<f32>(model_view[0].xyz, model_view[1].xyz, model_view[2].xyz));
    // MPR passes view-space vectors to its pixel shader. The viewer lights in
    // world space; convert back with the camera rotation before interpolation.
    return mat3x3<f32>(view_to_world[0].xyz, view_to_world[1].xyz, view_to_world[2].xyz) * normal_view;
}

fn mpr_vertex_tangent(frame: mat3x3<f32>, model: mat4x4<f32>, tangent: vec4<f32>) -> vec4<f32> {
    let det = dot(model[0].xyz, cross(model[1].xyz, model[2].xyz));
    // Source 66 transforms tangent.xyz with the SAME inverse transpose as the
    // normal, without vertex normalization. Zero determinant gives zero w.
    return vec4<f32>(frame * tangent.xyz, sign(det) * tangent.w);
}

fn mpr_vertex_color(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(2.0 * pow(abs(color.rgb), vec3<f32>(2.2)), color.a);
}

fn mpr_face_scale(light_scale: f32, front_facing: bool) -> f32 {
    return select(light_scale, 1.0, front_facing);
}

struct MprSurfaceChannels {
    diffuse: vec3<f32>,
    f0: vec3<f32>,
    roughness: f32,
};

fn mpr_surface_channels(base: vec3<f32>, metal: vec3<f32>, diffuse_color: vec3<f32>,
                        vertex_color: vec3<f32>, light_scale: f32, front_facing: bool) -> MprSurfaceChannels {
    let scale = mpr_face_scale(light_scale, front_facing);
    var result: MprSurfaceChannels;
    result.diffuse = base * (1.0 - metal.b) * metal.r * diffuse_color * vertex_color * abs(scale);
    result.f0 = (vec3<f32>(0.04) + metal.b * (base - vec3<f32>(0.04)))
        * select(0.0, 1.0, front_facing || scale <= 0.0);
    result.roughness = max(metal.g, 0.02);
    return result;
}

fn mpr_surface_alpha(base_alpha: f32, diffuse_alpha: f32, vertex_alpha: f32, single_map: bool) -> f32 {
    return base_alpha * base_alpha * diffuse_alpha * select(1.0, vertex_alpha, single_map);
}

fn mpr_layer_weight(primary_alpha: f32, overlay_alpha: f32, vertex_alpha: f32,
                    smoothness: f32, scales: vec4<f32>) -> f32 {
    let hp = clamp(primary_alpha * scales.x + scales.y, 0.0, 1.0);
    let ho = clamp(overlay_alpha * scales.z + scales.w, 0.0, 1.0);
    let x = (2.0 * vertex_alpha - 1.0) * (smoothness + 1.0) + ho - hp;
    let t = clamp((x + smoothness) / (2.0 * smoothness), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn mpr_tangent_normal(sampled: vec2<f32>, normal: vec3<f32>, tangent: vec4<f32>) -> vec3<f32> {
    let xy = sampled * (255.0 / 128.0) - vec2<f32>(1.0);
    let z = sqrt(1.0 - clamp(dot(xy, xy), 0.0, 1.0));
    let n = normalize(normal);
    let t = normalize(tangent.xyz);
    let b = cross(n, t) * tangent.w;
    return xy.x * t + xy.y * b + z * n;
}

fn mpr_layer_normal(primary: vec2<f32>, overlay: vec2<f32>, weight: f32,
                    normal: vec3<f32>, tangent0: vec4<f32>, tangent1: vec4<f32>) -> vec3<f32> {
    let p = mpr_tangent_normal(primary, normal, tangent0);
    let o = mpr_tangent_normal(overlay, normal, tangent1);
    return normalize(mix(p, o, weight));
}

fn mpr_single_normal(sampled: vec2<f32>, normal: vec3<f32>, tangent: vec4<f32>) -> vec3<f32> {
    // The single-map shader uses this vector directly: it has no final normalize.
    return mpr_tangent_normal(sampled, normal, tangent);
}
