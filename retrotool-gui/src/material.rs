use bevy::{
    pbr::*,
    prelude::*,
    reflect::TypeUuid,
    render::{mesh::*, render_resource::*},
};

// A "high" random id should be used for custom attributes to ensure consistent sorting and avoid collisions with other attributes.
// See the MeshVertexAttribute docs for more info.
const ATTRIBUTE_UV_1: MeshVertexAttribute =
    MeshVertexAttribute::new("Vertex_Uv_1", 988540917, VertexFormat::Float32x2);
const ATTRIBUTE_UV_2: MeshVertexAttribute =
    MeshVertexAttribute::new("Vertex_Uv_2", 988540918, VertexFormat::Float32x2);
const ATTRIBUTE_UV_3: MeshVertexAttribute =
    MeshVertexAttribute::new("Vertex_Uv_3", 988540919, VertexFormat::Float32x2);
const ATTRIBUTE_TANGENT_1: MeshVertexAttribute =
    MeshVertexAttribute::new("Vertex_Tangent_1", 988540920, VertexFormat::Float32x4);
const ATTRIBUTE_TANGENT_2: MeshVertexAttribute =
    MeshVertexAttribute::new("Vertex_Tangent_2", 988540921, VertexFormat::Float32x4);

// This is the struct that will be passed to your shader
#[derive(AsBindGroup, Debug, Clone, TypeUuid)]
#[uuid = "f690fdae-d598-45ab-8225-97e2a3f056e0"]
pub struct CustomMaterial {
    #[uniform(0)]
    pub base_color: Color,
    #[texture(1)]
    #[sampler(2)]
    pub base_color_texture_0: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub base_color_texture_1: Option<Handle<Image>>,
    #[texture(5)]
    #[sampler(6)]
    pub base_color_texture_2: Option<Handle<Image>>,
    #[uniform(0)]
    pub base_color_uv_0: u32,
    #[uniform(0)]
    pub base_color_uv_1: u32,
    #[uniform(0)]
    pub base_color_uv_2: u32,
    #[uniform(0)]
    pub base_color_l0: Color,
    #[uniform(0)]
    pub base_color_l1: Color,
    #[uniform(0)]
    pub base_color_l2: Color,
    // pub ican_color: Color,
    // pub ican_unmasked_color: Color,
    #[texture(7)]
    #[sampler(8)]
    pub normal_map_texture_0: Option<Handle<Image>>,
    #[texture(9)]
    #[sampler(10)]
    pub normal_map_texture_1: Option<Handle<Image>>,
    #[texture(11)]
    #[sampler(12)]
    pub normal_map_texture_2: Option<Handle<Image>>,
    #[uniform(0)]
    pub normal_map_uv_0: u32,
    #[uniform(0)]
    pub normal_map_uv_1: u32,
    #[uniform(0)]
    pub normal_map_uv_2: u32,
    #[uniform(0)]
    pub normal_map_l0: Color,
    #[uniform(0)]
    pub normal_map_l1: Color,
    #[uniform(0)]
    pub normal_map_l2: Color,
    #[texture(13)]
    #[sampler(14)]
    pub metallic_map_texture_0: Option<Handle<Image>>,
    #[texture(15)]
    #[sampler(16)]
    pub metallic_map_texture_1: Option<Handle<Image>>,
    #[texture(17)]
    #[sampler(18)]
    pub metallic_map_texture_2: Option<Handle<Image>>,
    #[uniform(0)]
    pub metallic_map_uv_0: u32,
    #[uniform(0)]
    pub metallic_map_uv_1: u32,
    #[uniform(0)]
    pub metallic_map_uv_2: u32,
    #[uniform(0)]
    pub metallic_map_l0: Color,
    #[uniform(0)]
    pub metallic_map_l1: Color,
    #[uniform(0)]
    pub metallic_map_l2: Color,
    #[texture(19)]
    #[sampler(20)]
    pub emissive_texture: Option<Handle<Image>>,
    #[uniform(0)]
    pub emissive_uv: u32,
    #[uniform(0)]
    pub emissive_color: Color,
}

impl Default for CustomMaterial {
    fn default() -> Self {
        Self {
            base_color: Color::WHITE,
            base_color_texture_0: None,
            base_color_texture_1: None,
            base_color_texture_2: None,
            base_color_uv_0: 0,
            base_color_uv_1: 0,
            base_color_uv_2: 0,
            base_color_l0: Color::NONE,
            base_color_l1: Color::NONE,
            base_color_l2: Color::NONE,
            normal_map_texture_0: None,
            normal_map_texture_1: None,
            normal_map_texture_2: None,
            normal_map_uv_0: 0,
            normal_map_uv_1: 0,
            normal_map_uv_2: 0,
            normal_map_l0: Color::NONE,
            normal_map_l1: Color::NONE,
            normal_map_l2: Color::NONE,
            metallic_map_texture_0: None,
            metallic_map_texture_1: None,
            metallic_map_texture_2: None,
            metallic_map_uv_0: 0,
            metallic_map_uv_1: 0,
            metallic_map_uv_2: 0,
            metallic_map_l0: Color::NONE,
            metallic_map_l1: Color::NONE,
            metallic_map_l2: Color::NONE,
            emissive_texture: None,
            emissive_uv: 0,
            emissive_color: Color::BLACK,
        }
    }
}

impl Material for CustomMaterial {
    fn vertex_shader() -> ShaderRef { "custom_material.wgsl".into() }

    fn fragment_shader() -> ShaderRef { "custom_material.wgsl".into() }

    fn specialize(
        _pipeline: &MaterialPipeline<Self>,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayout,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let mut shader_defs = Vec::new();
        let mut vertex_attributes = Vec::new();
        let mut add_attribute = |attr: MeshVertexAttribute, location: u32, define: &str| {
            if layout.contains(attr.clone()) {
                shader_defs.push(String::from(define));
                vertex_attributes.push(attr.at_shader_location(location));
            }
        };
        add_attribute(Mesh::ATTRIBUTE_POSITION, 0, "VERTEX_POSITIONS");
        add_attribute(Mesh::ATTRIBUTE_NORMAL, 1, "VERTEX_NORMALS");
        add_attribute(Mesh::ATTRIBUTE_UV_0, 2, "VERTEX_UVS_0");
        add_attribute(ATTRIBUTE_UV_1, 3, "VERTEX_UVS_1");
        add_attribute(ATTRIBUTE_UV_2, 4, "VERTEX_UVS_2");
        add_attribute(ATTRIBUTE_UV_3, 5, "VERTEX_UVS_3");
        add_attribute(Mesh::ATTRIBUTE_TANGENT, 6, "VERTEX_TANGENTS_0");
        add_attribute(ATTRIBUTE_TANGENT_1, 7, "VERTEX_TANGENTS_1");
        add_attribute(ATTRIBUTE_TANGENT_2, 8, "VERTEX_TANGENTS_2");
        add_attribute(Mesh::ATTRIBUTE_COLOR, 9, "VERTEX_COLORS");

        if layout.contains(Mesh::ATTRIBUTE_JOINT_INDEX)
            && layout.contains(Mesh::ATTRIBUTE_JOINT_WEIGHT)
        {
            shader_defs.push(String::from("SKINNED"));
            vertex_attributes.push(Mesh::ATTRIBUTE_JOINT_INDEX.at_shader_location(10));
            vertex_attributes.push(Mesh::ATTRIBUTE_JOINT_WEIGHT.at_shader_location(11));
        }

        let vertex_buffer_layout = layout.get_layout(&vertex_attributes)?;
        descriptor.vertex.buffers = vec![vertex_buffer_layout];
        descriptor.vertex.shader_defs.append(&mut shader_defs.clone());
        descriptor.fragment.as_mut().unwrap().shader_defs.append(&mut shader_defs);
        Ok(())
    }
}
