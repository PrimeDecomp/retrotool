#import bevy_pbr::mesh_view_bindings
#import bevy_pbr::mesh_bindings

#import bevy_pbr::utils
#import bevy_pbr::clustered_forward
#import bevy_pbr::lighting
#import bevy_pbr::pbr_ambient

#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping
#endif

#ifdef ENVIRONMENT_MAP
#import bevy_pbr::environment_map
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
};
@group(1) @binding(0)
var<uniform> material: CustomMaterial;
@group(1) @binding(1)
var base_color_texture_0: texture_2d<f32>;
@group(1) @binding(2)
var base_color_sampler_0: sampler;
@group(1) @binding(3)
var base_color_texture_1: texture_2d<f32>;
@group(1) @binding(4)
var base_color_sampler_1: sampler;
@group(1) @binding(5)
var base_color_texture_2: texture_2d<f32>;
@group(1) @binding(6)
var base_color_sampler_2: sampler;
@group(1) @binding(7)
var normal_map_texture_0: texture_2d<f32>;
@group(1) @binding(8)
var normal_map_sampler_0: sampler;
@group(1) @binding(9)
var normal_map_texture_1: texture_2d<f32>;
@group(1) @binding(10)
var normal_map_sampler_1: sampler;
@group(1) @binding(11)
var normal_map_texture_2: texture_2d<f32>;
@group(1) @binding(12)
var normal_map_sampler_2: sampler;
@group(1) @binding(13)
var metallic_map_texture_0: texture_2d<f32>;
@group(1) @binding(14)
var metallic_map_sampler_0: sampler;
@group(1) @binding(15)
var metallic_map_texture_1: texture_2d<f32>;
@group(1) @binding(16)
var metallic_map_sampler_1: sampler;
@group(1) @binding(17)
var metallic_map_texture_2: texture_2d<f32>;
@group(1) @binding(18)
var metallic_map_sampler_2: sampler;
@group(1) @binding(19)
var emissive_texture: texture_2d<f32>;
@group(1) @binding(20)
var emissive_sampler: sampler;

// NOTE: Bindings must come before functions that use them!
#import bevy_pbr::mesh_functions

struct VertexInput {
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
    var model = skin_model(in.joint_indices, in.joint_weights);
#else
    var model = mesh.model;
#endif
    out.position = mesh_position_local_to_clip(model, position);
    out.world_position = mesh_position_local_to_world(model, position);
#ifdef VERTEX_NORMALS
#ifdef SKINNED
    out.world_normal = skin_normals(model, in.normal);
#else
    out.world_normal = mesh_normal_local_to_world(in.normal);
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
#ifdef VERTEX_TANGENTS_0
    out.world_tangent_0 = mesh_tangent_local_to_world(mesh.model, in.tangent_0);
#endif
#ifdef VERTEX_TANGENTS_1
    out.world_tangent_1 = mesh_tangent_local_to_world(mesh.model, in.tangent_1);
#endif
#ifdef VERTEX_TANGENTS_2
    out.world_tangent_2 = mesh_tangent_local_to_world(mesh.model, in.tangent_2);
#endif
#ifdef VERTEX_COLORS
    out.color = in.color;
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
    color /= material.base_color_l0
        + material.base_color_l1
        + material.base_color_l2;
    return color;
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
    color /= material.normal_map_l0
        + material.normal_map_l1
        + material.normal_map_l2;
    return color;
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
    color /= material.metallic_map_l0
        + material.metallic_map_l1
        + material.metallic_map_l2;
    return color;
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
    Nt.z = sqrt(1.0 - Nt.x * Nt.x - Nt.y * Nt.y);
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
        V = normalize(vec3<f32>(view.view_proj[0].z, view.view_proj[1].z, view.view_proj[2].z));
    } else {
        // Only valid for a perpective projection
        V = normalize(view.world_position.xyz - world_position.xyz);
    }
    return V;
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
    output_color.a = 1.0;

    // Neubelt and Pettineo 2013, "Crafting a Next-gen Material Pipeline for The Order: 1886"
    let NdotV = max(dot(in.N, in.V), 0.0001);

    // Remapping [0,1] reflectance to F0
    // See https://google.github.io/filament/Filament.html#materialsystem/parameterization/remapping
    let reflectance = in.material.reflectance;
    let F0 = 0.16 * reflectance * reflectance * (1.0 - metallic) + output_color.rgb * metallic;

    // Diffuse strength inversely related to metallicity
    let diffuse_color = output_color.rgb * (1.0 - metallic);

    let R = reflect(-in.V, in.N);

    let f_ab = F_AB(perceptual_roughness, NdotV);

    var direct_light: vec3<f32> = vec3<f32>(0.0);

    let view_z = dot(vec4<f32>(
        view.inverse_view[0].z,
        view.inverse_view[1].z,
        view.inverse_view[2].z,
        view.inverse_view[3].z
    ), in.world_position);
    let cluster_index = fragment_cluster_index(in.frag_coord.xy, view_z, in.is_orthographic);
    let offset_and_counts = unpack_offset_and_counts(cluster_index);

    // Point lights (direct)
    for (var i: u32 = offset_and_counts[0]; i < offset_and_counts[0] + offset_and_counts[1]; i = i + 1u) {
        let light_id = get_light_id(i);
        var shadow: f32 = 1.0;
//        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
//                && (point_lights.data[light_id].flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
//            shadow = fetch_point_shadow(light_id, in.world_position, in.world_normal);
//        }
        let light_contrib = point_light(in.world_position.xyz, light_id, roughness, NdotV, in.N, in.V, R, F0, f_ab, diffuse_color);
        direct_light += light_contrib * shadow;
    }

    // Spot lights (direct)
    for (var i: u32 = offset_and_counts[0] + offset_and_counts[1]; i < offset_and_counts[0] + offset_and_counts[1] + offset_and_counts[2]; i = i + 1u) {
        let light_id = get_light_id(i);
        var shadow: f32 = 1.0;
//        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
//                && (point_lights.data[light_id].flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
//            shadow = fetch_spot_shadow(light_id, in.world_position, in.world_normal);
//        }
        let light_contrib = spot_light(in.world_position.xyz, light_id, roughness, NdotV, in.N, in.V, R, F0, f_ab, diffuse_color);
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
        var light_contrib = directional_light(i, roughness, NdotV, in.N, in.V, R, F0, f_ab, diffuse_color);
//#ifdef DIRECTIONAL_LIGHT_SHADOW_MAP_DEBUG_CASCADES
//        light_contrib = cascade_debug_visualization(light_contrib, i, view_z);
//#endif
        direct_light += light_contrib * shadow;
    }

    // Ambient light (indirect)
    var indirect_light = ambient_light(in.world_position, in.N, in.V, NdotV, diffuse_color, F0, perceptual_roughness, occlusion);

    // Environment map light (indirect)
#ifdef ENVIRONMENT_MAP
    let environment_light = environment_map_light(perceptual_roughness, roughness, diffuse_color, NdotV, f_ab, in.N, R, F0);
    indirect_light += (environment_light.diffuse * occlusion) + environment_light.specular;
#endif

    let emissive_light = emissive.rgb * output_color.a;

    // Total light
    output_color = vec4<f32>(
        direct_light + indirect_light + emissive_light,
        output_color.a
    );

    return output_color;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var in2: VertexOutput = in;

    var pbr_input = pbr_input_new();
    pbr_input.material.base_color = sample_base_color(&in2) * material.base_color;
//#ifdef VERTEX_COLORS
//    pbr_input.material.base_color *= in.color;
//#endif

    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = in.world_normal;
    pbr_input.is_orthographic = view.projection[3].w == 1.0;

    pbr_input.material.emissive = textureSample(
        emissive_texture,
        emissive_sampler,
        in_uv(&in2, material.emissive_uv)
    ) * material.emissive_color;

    let metl_map = sample_metallic_map(&in2);
    pbr_input.material.base_color *= metl_map.r; // AO
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
    pbr_input.V = calculate_view(in.world_position, pbr_input.is_orthographic);

    var output_color = pbr(pbr_input);
#ifdef TONEMAP_IN_SHADER
    output_color = tone_mapping(output_color);
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
