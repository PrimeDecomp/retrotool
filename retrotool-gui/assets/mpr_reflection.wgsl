// Recovered SingleVine/LayeredVine source66 reflection operations. Runtime
// probe selection, baked transforms, cube and original BRDF table are inputs.
// Mapped normal can be nonunit (SingleVine); do not normalize it here.
struct MprReflectionCoordinates {
    cube_direction: vec3<f32>,
    cube_lod: f32,
    brdf_uv: vec2<f32>,
};

fn mpr_reflection_coordinates(
    camera_to_fragment: vec3<f32>,
    normal: vec3<f32>,
    geometric_normal: vec3<f32>,
    baked_to_view: mat3x3<f32>,
    raw_roughness: f32,
    max_valid_mip_index: f32,
) -> MprReflectionCoordinates {
    let reflected = camera_to_fragment - 2.0 * normal * dot(normal, camera_to_fragment);
    let corrected = reflected + geometric_normal * clamp(dot(geometric_normal, -reflected), 0.0, 1.0);
    // Original shader multiplies by the transpose, without a general inverse.
    let baked = transpose(baked_to_view) * corrected;
    let major = max(max(abs(baked.x), abs(baked.y)), abs(baked.z));
    var result: MprReflectionCoordinates;
    result.cube_direction = baked / major;
    result.cube_lod = (1.0 - raw_roughness) * -max_valid_mip_index + max_valid_mip_index;
    result.brdf_uv = vec2<f32>(clamp(dot(normal, -camera_to_fragment), 0.0, 1.0), max(raw_roughness, 0.02));
    return result;
}

// Params: max valid mip, minimum intensity, maximum intensity, probe intensity.
// The source66 vines use a literal1 as the numerator of the intensity ratio.
fn mpr_vine_reflection_intensity(params: vec4<f32>) -> f32 {
    return params.y + (params.w - params.y) * clamp(1.0 / params.z, 0.0, 1.0);
}

fn mpr_vine_reflection_specular(cube_rgb: vec3<f32>, brdf_rg: vec2<f32>, f0: vec3<f32>, occlusion: f32, params: vec4<f32>) -> vec3<f32> {
    let scaled_cube = cube_rgb * (occlusion * mpr_vine_reflection_intensity(params));
    return scaled_cube * (f0 * brdf_rg.x + vec3<f32>(brdf_rg.y));
}
