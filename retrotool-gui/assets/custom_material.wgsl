#import "mpr_layered.wgsl"::{mpr_vertex_frame_to_world, mpr_vertex_tangent, mpr_vertex_color, mpr_face_scale, mpr_surface_channels, mpr_surface_alpha, mpr_layer_weight, mpr_layer_normal, mpr_single_normal}
#import "mpr_lighting.wgsl"::{mpr_combine_direct, mpr_direct_light}
#import bevy_pbr::{
    mesh_view_bindings::{view, lights, clustered_lights},
    mesh_view_types::POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE,
    mesh_functions::{get_world_from_local, mesh_position_local_to_clip, mesh_position_local_to_world, mesh_normal_local_to_world, mesh_tangent_local_to_world},
    clustered_forward::{view_fragment_cluster_index, unpack_clusterable_object_index_ranges, get_clusterable_object_id},
    lighting::{LightingInput, LayerLightingInput, getDistanceAttenuation, perceptualRoughnessToRoughness, F_AB, point_light, spot_light, directional_light},
    ambient::ambient_light,
}
#ifdef SKINNED
#import bevy_pbr::skinning::{skin_model, skin_normals}
#endif
#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping::{tone_mapping, screen_space_dither}
#endif
#ifdef ENVIRONMENT_MAP
#import bevy_pbr::environment_map::environment_map_light
#endif

struct CustomMaterial {
    base_color: vec4<f32>,
    base_color_uv_0: u32,
    base_color_uv_1: u32,
    base_color_uv_2: u32,
    base_color_l0: vec4<f32>,
    base_color_l1: vec4<f32>,
    base_color_l2: vec4<f32>,
    normal_map_uv_0: u32,
    normal_map_uv_1: u32,
    normal_map_uv_2: u32,
    normal_map_l0: vec4<f32>,
    normal_map_l1: vec4<f32>,
    normal_map_l2: vec4<f32>,
    metallic_map_uv_0: u32,
    metallic_map_uv_1: u32,
    metallic_map_uv_2: u32,
    metallic_map_l0: vec4<f32>,
    metallic_map_l1: vec4<f32>,
    metallic_map_l2: vec4<f32>,
    emissive_uv: u32,
    emissive_color: vec4<f32>,
    emissive_unmasked_color: vec4<f32>,
    layer_smoothness: f32,
    layer_scales: vec4<f32>,
    light_scale: f32,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> material: CustomMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var base_color_texture_0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2)
var base_color_sampler_0: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3)
var base_color_texture_1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4)
var base_color_sampler_1: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(5)
var base_color_texture_2: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6)
var base_color_sampler_2: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(7)
var normal_map_texture_0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(8)
var normal_map_sampler_0: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(9)
var normal_map_texture_1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(10)
var normal_map_sampler_1: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(11)
var normal_map_texture_2: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(12)
var normal_map_sampler_2: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(13)
var metallic_map_texture_0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(14)
var metallic_map_sampler_0: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(15)
var metallic_map_texture_1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(16)
var metallic_map_sampler_1: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(17)
var metallic_map_texture_2: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(18)
var metallic_map_sampler_2: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(19)
var emissive_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(20)
var emissive_sampler: sampler;

struct VertexInput {
    @builtin(instance_index) instance_index: u32,
#ifdef VERTEX_POSITIONS
    @location(0) position: vec3<f32>,
#endif
#ifdef VERTEX_NORMALS
    @location(1) normal: vec3<f32>,
#endif
#ifdef VERTEX_UVS_0
    @location(2) uv_0: vec2<f32>,
#endif
#ifdef VERTEX_UVS_1
    @location(3) uv_1: vec2<f32>,
#endif
#ifdef VERTEX_UVS_2
    @location(4) uv_2: vec2<f32>,
#endif
#ifdef VERTEX_UVS_3
    @location(5) uv_3: vec2<f32>,
#endif
#ifdef VERTEX_TANGENTS_0
    @location(6) tangent_0: vec4<f32>,
#endif
#ifdef VERTEX_TANGENTS_1
    @location(7) tangent_1: vec4<f32>,
#endif
#ifdef VERTEX_TANGENTS_2
    @location(8) tangent_2: vec4<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(9) color: vec4<f32>,
#endif
#ifdef SKINNED
    @location(10) joint_indices: vec4<u32>,
    @location(11) joint_weights: vec4<f32>,
#endif
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
#ifdef VERTEX_NORMALS
    @location(1) world_normal: vec3<f32>,
#endif
#ifdef VERTEX_UVS_0
    @location(2) uv_0: vec2<f32>,
#endif
#ifdef VERTEX_UVS_1
    @location(3) uv_1: vec2<f32>,
#endif
#ifdef VERTEX_UVS_2
    @location(4) uv_2: vec2<f32>,
#endif
#ifdef VERTEX_TANGENTS_0
    @location(5) world_tangent_0: vec4<f32>,
#endif
#ifdef VERTEX_TANGENTS_1
    @location(6) world_tangent_1: vec4<f32>,
#endif
#ifdef VERTEX_TANGENTS_2
    @location(7) world_tangent_2: vec4<f32>,
#endif
#ifdef VERTEX_UVS_3
    @location(9) uv_3: vec2<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(8) color: vec4<f32>,
#endif
};

@vertex
fn vertex(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
#ifdef VERTEX_POSITIONS
    let position = vec4<f32>(in.position, 1.0);
#else
    let position = vec4<f32>(0.0, 0.0, 0.0, 1.0);
#endif
#ifdef SKINNED
    var model = skin_model(in.joint_indices, in.joint_weights, in.instance_index);
#else
    var model = get_world_from_local(in.instance_index);
#endif
    out.position = mesh_position_local_to_clip(model, position);
    out.world_position = mesh_position_local_to_world(model, position);
#ifdef MPR_RECOVERED_SURFACE
    let mpr_frame = mpr_vertex_frame_to_world(model, view.view_from_world, view.world_from_view);
#endif
#ifdef VERTEX_NORMALS
#ifdef SKINNED
    out.world_normal = skin_normals(model, in.normal);
#else
#ifdef MPR_RECOVERED_SURFACE
    out.world_normal = mpr_frame * in.normal;
#else
    out.world_normal = mesh_normal_local_to_world(in.normal, in.instance_index);
#endif
#endif
#endif
#ifdef VERTEX_UVS_0
    out.uv_0 = in.uv_0;
#endif
#ifdef VERTEX_UVS_1
    out.uv_1 = in.uv_1;
#endif
#ifdef VERTEX_UVS_2
    out.uv_2 = in.uv_2;
#endif
#ifdef VERTEX_UVS_3
    out.uv_3 = in.uv_3;
#endif
#ifdef VERTEX_TANGENTS_0
#ifdef MPR_RECOVERED_SURFACE
    out.world_tangent_0 = mpr_vertex_tangent(mpr_frame, model, in.tangent_0);
#else
    out.world_tangent_0 = mesh_tangent_local_to_world(model, in.tangent_0, in.instance_index);
#endif
#endif
#ifdef VERTEX_TANGENTS_1
#ifdef MPR_RECOVERED_SURFACE
    out.world_tangent_1 = mpr_vertex_tangent(mpr_frame, model, in.tangent_1);
#else
    out.world_tangent_1 = mesh_tangent_local_to_world(model, in.tangent_1, in.instance_index);
#endif
#endif
#ifdef VERTEX_TANGENTS_2
    out.world_tangent_2 = mesh_tangent_local_to_world(model, in.tangent_2, in.instance_index);
#endif
#ifdef VERTEX_COLORS
    out.color = in.color;
#ifdef MPR_RECOVERED_SURFACE
    out.color = mpr_vertex_color(in.color);
#endif
#endif
    return out;
}

fn in_uv(in: ptr<function, VertexOutput>, idx: u32) -> vec2<f32> {
    switch (idx) {
#ifdef VERTEX_UVS_0
        case 0u: {
            return (*in).uv_0;
        }
#endif
#ifdef VERTEX_UVS_1
        case 1u: {
            return (*in).uv_1;
        }
#endif
#ifdef VERTEX_UVS_2
        case 2u: {
            return (*in).uv_2;
        }
#endif
#ifdef VERTEX_UVS_3
        case 3u: { return (*in).uv_3; }
#endif
        default: {
            return vec2<f32>(0.0);
        }
    }
}

fn sample_base_color(in: ptr<function, VertexOutput>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    color += textureSample(
        base_color_texture_0,
        base_color_sampler_0,
        in_uv(in, material.base_color_uv_0)
    ) * material.base_color_l0;
    color += textureSample(
        base_color_texture_1,
        base_color_sampler_1,
        in_uv(in, material.base_color_uv_1)
    ) * material.base_color_l1;
    color += textureSample(
        base_color_texture_2,
        base_color_sampler_2,
        in_uv(in, material.base_color_uv_2)
    ) * material.base_color_l2;
    let weights = material.base_color_l0 + material.base_color_l1 + material.base_color_l2;
    return select(vec4<f32>(1.0), color / max(weights, vec4<f32>(1e-20)), weights > vec4<f32>(0.0));
}

fn sample_normal_map(in: ptr<function, VertexOutput>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    color += textureSample(
        normal_map_texture_0,
        normal_map_sampler_0,
        in_uv(in, material.normal_map_uv_0)
    ) * material.normal_map_l0;
    color += textureSample(
        normal_map_texture_1,
        normal_map_sampler_1,
        in_uv(in, material.normal_map_uv_1)
    ) * material.normal_map_l1;
    color += textureSample(
        normal_map_texture_2,
        normal_map_sampler_2,
        in_uv(in, material.normal_map_uv_2)
    ) * material.normal_map_l2;
    let weights = material.normal_map_l0 + material.normal_map_l1 + material.normal_map_l2;
    return select(vec4<f32>(0.5, 0.5, 1.0, 1.0), color / max(weights, vec4<f32>(1e-20)), weights > vec4<f32>(0.0));
}

fn sample_metallic_map(in: ptr<function, VertexOutput>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    color += textureSample(
        metallic_map_texture_0,
        metallic_map_sampler_0,
        in_uv(in, material.metallic_map_uv_0)
    ) * material.metallic_map_l0;
    color += textureSample(
        metallic_map_texture_1,
        metallic_map_sampler_1,
        in_uv(in, material.metallic_map_uv_1)
    ) * material.metallic_map_l1;
    color += textureSample(
        metallic_map_texture_2,
        metallic_map_sampler_2,
        in_uv(in, material.metallic_map_uv_2)
    ) * material.metallic_map_l2;
    let weights = material.metallic_map_l0 + material.metallic_map_l1 + material.metallic_map_l2;
    return select(vec4<f32>(1.0, 1.0, 0.0, 1.0), color / max(weights, vec4<f32>(1e-20)), weights > vec4<f32>(0.0));
}

fn apply_normal_mapping(
    world_normal: vec3<f32>,
#ifdef VERTEX_TANGENTS_0
    world_tangent: vec4<f32>,
    in: ptr<function, VertexOutput>,
#endif
) -> vec3<f32> {
    // NOTE: The mikktspace method of normal mapping explicitly requires that the world normal NOT
    // be re-normalized in the fragment shader. This is primarily to match the way mikktspace
    // bakes vertex tangents and normal maps so that this is the exact inverse. Blender, Unity,
    // Unreal Engine, Godot, and more all use the mikktspace method. Do not change this code
    // unless you really know what you are doing.
    // http://www.mikktspace.com/
    var N: vec3<f32> = world_normal;

#ifdef VERTEX_TANGENTS_0
    // NOTE: The mikktspace method of normal mapping explicitly requires that these NOT be
    // normalized nor any Gram-Schmidt applied to ensure the vertex normal is orthogonal to the
    // vertex tangent! Do not change this code unless you really know what you are doing.
    // http://www.mikktspace.com/
    var T: vec3<f32> = world_tangent.xyz;
    var B: vec3<f32> = world_tangent.w * cross(N, T);
    // Nt is the tangent-space normal.
    var Nt = sample_normal_map(in).rgb;
    // Only use the xy components and derive z for 2-component normal maps.
    Nt = vec3<f32>(Nt.rg * 2.0 - 1.0, 0.0);
    Nt.z = sqrt(max(0.0, 1.0 - Nt.x * Nt.x - Nt.y * Nt.y));
    // Normal maps authored for DirectX require flipping the y component
    // Nt.y = -Nt.y;
    // NOTE: The mikktspace method of normal mapping applies maps the tangent-space normal from
    // the normal map texture in this way to be an EXACT inverse of how the normal map baker
    // calculates the normal maps so there is no error introduced. Do not change this code
    // unless you really know what you are doing.
    // http://www.mikktspace.com/
    N = Nt.x * T + Nt.y * B + Nt.z * N;
#endif

    return normalize(N);
}

struct StandardMaterial {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    perceptual_roughness: f32,
    metallic: f32,
    reflectance: f32,
    // 'flags' is a bit field indicating various options. u32 is 32 bits so we have up to 32 options.
    flags: u32,
    alpha_cutoff: f32,
};

const STANDARD_MATERIAL_FLAGS_BASE_COLOR_TEXTURE_BIT: u32         = 1u;
const STANDARD_MATERIAL_FLAGS_EMISSIVE_TEXTURE_BIT: u32           = 2u;
const STANDARD_MATERIAL_FLAGS_METALLIC_ROUGHNESS_TEXTURE_BIT: u32 = 4u;
const STANDARD_MATERIAL_FLAGS_OCCLUSION_TEXTURE_BIT: u32          = 8u;
const STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT: u32               = 16u;
const STANDARD_MATERIAL_FLAGS_UNLIT_BIT: u32                      = 32u;
const STANDARD_MATERIAL_FLAGS_ALPHA_MODE_OPAQUE: u32              = 64u;
const STANDARD_MATERIAL_FLAGS_ALPHA_MODE_MASK: u32                = 128u;
const STANDARD_MATERIAL_FLAGS_ALPHA_MODE_BLEND: u32               = 256u;
const STANDARD_MATERIAL_FLAGS_TWO_COMPONENT_NORMAL_MAP: u32       = 512u;
const STANDARD_MATERIAL_FLAGS_FLIP_NORMAL_MAP_Y: u32              = 1024u;

// Creates a StandardMaterial with default values
fn standard_material_new() -> StandardMaterial {
    var material: StandardMaterial;

    // NOTE: Keep in-sync with src/pbr_material.rs!
    material.base_color = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    material.emissive = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    material.perceptual_roughness = 0.089;
    material.metallic = 0.01;
    material.reflectance = 0.5;
    material.flags = STANDARD_MATERIAL_FLAGS_ALPHA_MODE_OPAQUE;
    material.alpha_cutoff = 0.5;

    return material;
}

struct PbrInput {
    material: StandardMaterial,
    occlusion: f32,
    surface_diffuse: vec3<f32>,
    surface_f0: vec3<f32>,
    frag_coord: vec4<f32>,
    world_position: vec4<f32>,
    // Normalized world normal used for shadow mapping as normal-mapping is not used for shadow
    // mapping
    world_normal: vec3<f32>,
    // Normalized normal-mapped world normal used for lighting
    N: vec3<f32>,
    // Normalized view vector in world space, pointing from the fragment world position toward the
    // view world position
    V: vec3<f32>,
    is_orthographic: bool,
};

// Creates a PbrInput with default values
fn pbr_input_new() -> PbrInput {
    var pbr_input: PbrInput;

    pbr_input.material = standard_material_new();
    pbr_input.occlusion = 1.0;
    pbr_input.surface_diffuse = vec3<f32>(0.0);
    pbr_input.surface_f0 = vec3<f32>(0.0);

    pbr_input.frag_coord = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    pbr_input.world_position = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    pbr_input.world_normal = vec3<f32>(0.0, 0.0, 1.0);

    pbr_input.is_orthographic = false;

    pbr_input.N = vec3<f32>(0.0, 0.0, 1.0);
    pbr_input.V = vec3<f32>(1.0, 0.0, 0.0);

    return pbr_input;
}

// NOTE: Correctly calculates the view vector depending on whether
// the projection is orthographic or perspective.
fn calculate_view(
    world_position: vec4<f32>,
    is_orthographic: bool,
) -> vec3<f32> {
    var V: vec3<f32>;
    if (is_orthographic) {
        // Orthographic view vector
        V = normalize(vec3<f32>(view.clip_from_world[0].z, view.clip_from_world[1].z, view.clip_from_world[2].z));
    } else {
        // Only valid for a perpective projection
        V = normalize(view.world_position.xyz - world_position.xyz);
    }
    return V;
}

// Viewer light placement/intensity/range remain Bevy inputs. Use the traced
// local-light BRDF for these preview lights, with CLight's default roughness0.
// Runtime MPR records instead use mpr_local_light's cubic/linear cone falloff.
fn mpr_preview_local_light(in: PbrInput, light_id: u32, spot: bool) -> vec3<f32> {
    let light = clustered_lights.data[light_id];
    let delta = light.position_radius.xyz - in.world_position.xyz;
    let direction = normalize(delta);
    var attenuation = getDistanceAttenuation(dot(delta, delta), light.color_inverse_square_range.w);
    if (spot) {
        var spot_direction = vec3<f32>(light.light_custom_data.x, 0.0, light.light_custom_data.y);
        spot_direction.y = sqrt(max(0.0, 1.0 - dot(spot_direction, spot_direction)));
        if ((light.flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u) {
            spot_direction.y = -spot_direction.y;
        }
        let cone = clamp(dot(-spot_direction, direction) * light.light_custom_data.z + light.light_custom_data.w, 0.0, 1.0);
        attenuation *= cone * cone;
    }
    return mpr_combine_direct(mpr_direct_light(in.N, in.V, direction,
        in.material.perceptual_roughness, in.surface_f0,
        light.color_inverse_square_range.rgb * attenuation, 0.0), in.surface_diffuse);
}

fn pbr(
    in: PbrInput,
) -> vec4<f32> {
    var output_color: vec4<f32> = in.material.base_color;

    // TODO use .a for exposure compensation in HDR
    let emissive = in.material.emissive;

    // calculate non-linear roughness from linear perceptualRoughness
    let metallic = in.material.metallic;
    let perceptual_roughness = in.material.perceptual_roughness;
    let roughness = perceptualRoughnessToRoughness(perceptual_roughness);

    let occlusion = in.occlusion;

    // output_color = alpha_discard(in.material, output_color);
    // output_color.a = 1.0;

    // Neubelt and Pettineo 2013, "Crafting a Next-gen Material Pipeline for The Order: 1886"
    let NdotV = max(dot(in.N, in.V), 0.0001);

#ifdef MPR_RECOVERED_SURFACE
    let F0 = in.surface_f0;
    let diffuse_color = in.surface_diffuse;
#else
    // Remapping [0,1] reflectance to F0
    // See https://google.github.io/filament/Filament.html#materialsystem/parameterization/remapping
    let reflectance = in.material.reflectance;
    let F0 = 0.16 * reflectance * reflectance * (1.0 - metallic) + output_color.rgb * metallic;

    // Diffuse strength inversely related to metallicity
    let diffuse_color = output_color.rgb * (1.0 - metallic);
#endif

    let R = reflect(-in.V, in.N);

    let f_ab = F_AB(perceptual_roughness, NdotV);
    var lighting: LightingInput;
    lighting.layers[0] = LayerLightingInput(in.N, R, NdotV, perceptual_roughness, roughness);
    lighting.P = in.world_position.xyz;
    lighting.V = in.V;
    lighting.diffuse_color = diffuse_color;
    lighting.F_ab = f_ab;
#ifdef MPR_RECOVERED_SURFACE
    // Recovered surfaces supply F0 and diffuse independently of Bevy's metalness model.
    lighting.F0_dielectric = F0;
    lighting.F0_metallic = F0;
    lighting.metallic = 0.0;
#else
    lighting.F0_dielectric = vec3(0.16 * reflectance * reflectance);
    lighting.F0_metallic = output_color.rgb;
    lighting.metallic = metallic;
#endif

    var direct_light: vec3<f32> = vec3<f32>(0.0);

    let view_z = dot(vec4<f32>(
        view.world_from_view[0].z,
        view.world_from_view[1].z,
        view.world_from_view[2].z,
        view.world_from_view[3].z
    ), in.world_position);
    let cluster_index = view_fragment_cluster_index(in.frag_coord.xy, view_z, in.is_orthographic);
    var ranges = unpack_clusterable_object_index_ranges(cluster_index);

    // Point lights (direct)
    for (var i: u32 = ranges.first_point_light_index_offset; i < ranges.first_spot_light_index_offset; i = i + 1u) {
        let light_id = get_clusterable_object_id(i);
        var shadow: f32 = 1.0;
//        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
//                && (clustered_lights.data[light_id].flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
//            shadow = fetch_point_shadow(light_id, in.world_position, in.world_normal);
//        }
#ifdef MPR_RECOVERED_SURFACE
        let light_contrib = mpr_preview_local_light(in, light_id, false);
#else
        let light_contrib = point_light(light_id, &lighting, true, false);
#endif
        direct_light += light_contrib * shadow;
    }

    // Spot lights (direct)
    for (var i: u32 = ranges.first_spot_light_index_offset; i < ranges.first_reflection_probe_index_offset; i = i + 1u) {
        let light_id = get_clusterable_object_id(i);
        var shadow: f32 = 1.0;
//        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
//                && (clustered_lights.data[light_id].flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
//            shadow = fetch_spot_shadow(light_id, in.world_position, in.world_normal);
//        }
#ifdef MPR_RECOVERED_SURFACE
        let light_contrib = mpr_preview_local_light(in, light_id, true);
#else
        let light_contrib = spot_light(light_id, &lighting, true);
#endif
        direct_light += light_contrib * shadow;
    }

    // Directional lights (direct)
    let n_directional_lights = lights.n_directional_lights;
    for (var i: u32 = 0u; i < n_directional_lights; i = i + 1u) {
        var shadow: f32 = 1.0;
//        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
//                && (lights.directional_lights[i].flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
//            shadow = fetch_directional_shadow(i, in.world_position, in.world_normal, view_z);
//        }
#ifdef MPR_RECOVERED_SURFACE
        let preview_light = lights.directional_lights[i];
        var light_contrib = mpr_combine_direct(mpr_direct_light(in.N, in.V,
            preview_light.direction_to_light.xyz, perceptual_roughness, F0,
            preview_light.color.rgb, 0.0), diffuse_color);
#else
        var light_contrib = directional_light(i, &lighting, true);
#endif
//#ifdef DIRECTIONAL_LIGHT_SHADOW_MAP_DEBUG_CASCADES
//        light_contrib = cascade_debug_visualization(light_contrib, i, view_z);
//#endif
        direct_light += light_contrib * shadow;
    }

    // Ambient light (indirect)
    var indirect_light = ambient_light(in.world_position, in.N, in.V, NdotV, diffuse_color, F0, perceptual_roughness, vec3(occlusion));

    // Environment map light (indirect)
#ifdef ENVIRONMENT_MAP
    let environment_light = environment_map_light(&lighting, &ranges, false);
    indirect_light += (environment_light.diffuse * occlusion) + environment_light.specular;
#endif

#ifdef MPR_RECOVERED_SURFACE
    // These two permutations add ICNC + ICMC independently of output alpha.
    let emissive_light = emissive.rgb;
#else
    let emissive_light = emissive.rgb * output_color.a;
#endif

    // Preview lights use normalized values (ambient 0.05, environment 1.0).
    // Do not apply Bevy's physical-camera exposure to this legacy preview.
    output_color = vec4<f32>(
        direct_light + indirect_light + emissive_light,
        output_color.a
    );

    return output_color;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    var in2: VertexOutput = in;

    var pbr_input = pbr_input_new();
#ifdef MPR_RECOVERED_SURFACE
    let primary = textureSample(base_color_texture_0, base_color_sampler_0, in_uv(&in2, material.base_color_uv_0));
#ifdef MPR_RECOVERED_LAYERED
    let overlay = textureSample(base_color_texture_1, base_color_sampler_1, in_uv(&in2, material.base_color_uv_1));
    let layer_weight = mpr_layer_weight(primary.a, overlay.a, in.color.a, material.layer_smoothness, material.layer_scales);
    let base_sample = mix(primary, overlay, layer_weight);
    let alpha = mpr_surface_alpha(base_sample.a, material.base_color.a, in.color.a, false);
#else
    let base_sample = primary;
    let alpha = mpr_surface_alpha(base_sample.a, material.base_color.a, in.color.a, true);
#endif
    pbr_input.material.base_color = vec4<f32>(base_sample.rgb, alpha);
#else
    pbr_input.material.base_color = sample_base_color(&in2) * material.base_color;
#endif
//#ifdef VERTEX_COLORS
//    pbr_input.material.base_color *= in.color;
//#endif

    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
#ifdef VERTEX_NORMALS
    pbr_input.world_normal = in.world_normal;
#endif
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;

#ifdef MPR_RECOVERED_SURFACE
    pbr_input.material.emissive = material.emissive_color + material.emissive_unmasked_color;
#else
    pbr_input.material.emissive = textureSample(
        emissive_texture,
        emissive_sampler,
        in_uv(&in2, material.emissive_uv)
    ) * material.emissive_color;
#endif

#ifdef MPR_RECOVERED_SURFACE
#ifdef MPR_RECOVERED_LAYERED
    let metl_map = mix(
        textureSample(metallic_map_texture_0, metallic_map_sampler_0, in_uv(&in2, material.metallic_map_uv_0)),
        textureSample(metallic_map_texture_1, metallic_map_sampler_1, in_uv(&in2, material.metallic_map_uv_1)), layer_weight);
    pbr_input.N = mpr_layer_normal(
        textureSample(normal_map_texture_0, normal_map_sampler_0, in_uv(&in2, material.normal_map_uv_0)).rg,
        textureSample(normal_map_texture_1, normal_map_sampler_1, in_uv(&in2, material.normal_map_uv_1)).rg,
        layer_weight, in.world_normal, in.world_tangent_0, in.world_tangent_1);
#else
    let metl_map = textureSample(metallic_map_texture_0, metallic_map_sampler_0, in_uv(&in2, material.metallic_map_uv_0));
    pbr_input.N = mpr_single_normal(
        textureSample(normal_map_texture_0, normal_map_sampler_0, in_uv(&in2, material.normal_map_uv_0)).rg,
        in.world_normal, in.world_tangent_0);
#endif
    let channels = mpr_surface_channels(base_sample.rgb, metl_map.rgb, material.base_color.rgb,
        in.color.rgb, material.light_scale, front_facing);
    pbr_input.surface_diffuse = channels.diffuse;
    pbr_input.surface_f0 = channels.f0;
    pbr_input.material.perceptual_roughness = channels.roughness;
    pbr_input.material.metallic = metl_map.b;
    pbr_input.N *= sign(mpr_face_scale(material.light_scale, front_facing));
#else
    let metl_map = sample_metallic_map(&in2);
    pbr_input.material.base_color *= metl_map.r; // Legacy approximation.
    pbr_input.material.perceptual_roughness = metl_map.g;
    pbr_input.material.metallic = metl_map.b;

#ifdef VERTEX_NORMALS
    pbr_input.N = apply_normal_mapping(
        in.world_normal,
#ifdef VERTEX_TANGENTS_0
        in.world_tangent_0,
        &in2,
#endif
    );
#endif
#endif
    pbr_input.V = calculate_view(in.world_position, pbr_input.is_orthographic);

    var output_color = pbr(pbr_input);
#ifdef TONEMAP_IN_SHADER
    output_color = tone_mapping(output_color, view.color_grading);
#endif
#ifdef DEBAND_DITHER
    var output_rgb = output_color.rgb;
    output_rgb = pow(output_rgb, vec3<f32>(1.0 / 2.2));
    output_rgb = output_rgb + screen_space_dither(in.position.xy);
    // This conversion back to linear space is required because our output texture format is
    // SRGB; the GPU will assume our output is linear and will apply an SRGB conversion.
    output_rgb = pow(output_rgb, vec3<f32>(2.2));
    output_color = vec4(output_rgb, output_color.a);
#endif
    return output_color;
}
