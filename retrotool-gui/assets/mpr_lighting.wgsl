// MPR PhysicallyBased 0b8c1a78, exact LayeredVine/SingleVine source66 loops.
// Runtime record packed by CLightMaterialData::SCachedLight::Initialize,
// 0x710016b5c8. All positions/directions must use the same coordinate space.
// Runtime cluster prefix. These are pixel sizes and native view-space depth,
// independent of the preview renderer's own cluster implementation.
struct MprClusterParams {
    size: vec4<f32>,
    depth: vec4<f32>,
    index: vec4<u32>,
};

// Source66 TLDS.LZ coordinates. The original programs do not clamp the final
// coordinate to the texture extent; the far endpoint may equal slice count.
// Sample-adjusted pixel coordinates are required. This helper does not perform
// the driver's sample interpolation or the integer texture fetch.
fn mpr_cluster_mask_coordinate(params: MprClusterParams, pixel: vec2<f32>, view_z: f32) -> vec2<i32> {
    let tile = vec2<i32>(pixel * params.size.xy);
    let distance = clamp(-view_z - params.depth.z, 0.0, params.depth.w);
    var slice = 0.0;
    if (distance > 0.0) {
        slice = exp2(log2(distance) * params.depth.x) * params.depth.y;
    }
    return vec2<i32>(tile.x + tile.y * i32(params.index.y), i32(slice));
}

struct MprLight {
    color_roughness: vec4<f32>,
    position_cone_scale: vec4<f32>,
    direction_cone_bias: vec4<f32>,
    distance_scale_bias: vec4<f32>,
    distance_polynomial: vec4<f32>,
};

// ClusteredCullRaster sources75/76, before writing integer R/RG masks. The
// plane convention is dot(normal,point)=w, with low-inclusive/high-exclusive
// tangency. Do not replace this with symmetric <= AABB intersection.
fn mpr_cluster_sphere_visible(sphere: vec4<f32>, planes: array<vec4<f32>,6>) -> bool {
    return dot(planes[0].xyz, sphere.xyz) >= planes[0].w - sphere.w
        && dot(planes[1].xyz, sphere.xyz) < planes[1].w + sphere.w
        && dot(planes[2].xyz, sphere.xyz) >= planes[2].w - sphere.w
        && dot(planes[3].xyz, sphere.xyz) < planes[3].w + sphere.w
        && dot(planes[4].xyz, sphere.xyz) >= planes[4].w - sphere.w
        && dot(planes[5].xyz, sphere.xyz) < planes[5].w + sphere.w;
}

struct MprDirectLighting {
    diffuse_irradiance: vec3<f32>,
    specular: vec3<f32>,
};

fn mpr_direct_light(normal: vec3<f32>, view_direction: vec3<f32>, light_direction: vec3<f32>,
                    roughness: f32, f0: vec3<f32>, radiance: vec3<f32>, light_roughness: f32) -> MprDirectLighting {
    let half_vector = normalize(light_direction + view_direction);
    let nv = clamp(dot(normal, view_direction), 0.0, 1.0);
    let nl = clamp(dot(normal, light_direction), 0.0, 1.0);
    let nh = clamp(dot(normal, half_vector), 0.0, 1.0);
    let lh = clamp(dot(light_direction, half_vector), 0.0, 1.0);
    let r = roughness + (1.0 - roughness) * light_roughness;
    let r2 = r * r;
    let r4 = r2 * r2;
    let k = 0.5 * r2;
    let denominator = (r4 - 1.0) * nh * nh + 1.0;
    let distribution = r4 / max(3.1415927410125732422 * denominator * denominator, r4 * 0.000001);
    let visibility = 1.0 / (max(nv * (1.0 - k) + k, 0.000001) * max(nl * (1.0 - k) + k, 0.000001));
    let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - lh, 5.0);
    var result: MprDirectLighting;
    result.diffuse_irradiance = radiance * nl;
    result.specular = result.diffuse_irradiance * fresnel * (0.25 * distribution * visibility);
    return result;
}

fn mpr_local_light(light: MprLight, position: vec3<f32>, normal: vec3<f32>, view_direction: vec3<f32>,
                   roughness: f32, f0: vec3<f32>) -> MprDirectLighting {
    let delta = light.position_cone_scale.xyz - position;
    let distance = length(delta);
    let direction = delta / distance;
    let t = clamp(distance * light.distance_scale_bias.x + light.distance_scale_bias.y, 0.0, 1.0);
    let p = light.distance_polynomial;
    let falloff = ((p.x * t + p.y) * t + p.z) * t + p.w;
    let cone = clamp(dot(direction, light.direction_cone_bias.xyz) * light.position_cone_scale.w
        + light.direction_cone_bias.w, 0.0, 1.0);
    return mpr_direct_light(normal, view_direction, direction, roughness, f0,
        light.color_roughness.rgb * (falloff * cone), light.color_roughness.w);
}

fn mpr_combine_direct(light: MprDirectLighting, diffuse_color: vec3<f32>) -> vec3<f32> {
    return diffuse_color * light.diffuse_irradiance * 0.31830987334251403809 + light.specular;
}
