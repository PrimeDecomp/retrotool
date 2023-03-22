#import bevy_render::view

@group(0) @binding(0)
var<uniform> view: View;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) near: vec3<f32>,
    @location(1) far: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

// Adapted from https://github.com/lain-dono/shaderlab/blob/fb496dc0178b955330c1cd4a8590c44b4a9b9cd9/src/scene/gizmo.wgsl#L36
// Originally from https://asliceofrendering.com/scene%20helper/2020/01/05/InfiniteGrid/

fn unproject_point(x: f32, y: f32, z: f32) -> vec3<f32> {
    let unprojected = view.inverse_view_proj * vec4<f32>(x, y, z, 1.0);
    return unprojected.xyz / unprojected.w;
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let u = f32((index << 1u) & 2u) - 1.0;
    let v = 1.0 - f32(index & 2u);
    return VertexOutput(
        vec4<f32>(u, v, 0.0, 1.0),
        unproject_point(u, v, 1.000),
        unproject_point(u, v, 0.0000001),
        vec2<f32>(u, v),
    );
}

fn grid(pos: vec3<f32>, scale: f32, axis: bool) -> vec4<f32> {
    let coord = pos.xz * scale; // use the scale variable to set the distance between the lines
    let derivative = fwidth(coord);
    let grid = abs(fract(coord - 0.5) - 0.5) / derivative;
    let grid_line = min(grid.x, grid.y);
    let minimumz = min(derivative.y, 1.0);
    let minimumx = min(derivative.x, 1.0);
    let alpha = 1.0 - min(grid_line, 1.0);
    var color = vec4<f32>(0.01) * alpha;
    if (axis) {
        let extra = 1.0 / scale;
        // z axis
        if (pos.x > -extra * minimumx && pos.x < extra * minimumx) {
            color.x = 0.0;
            color.y = 0.0;
            color.z = 0.1 * alpha;
        }
        // x axis
        if (pos.z > -extra * minimumz && pos.z < extra * minimumz) {
            color.x = 0.1 * alpha;
            color.y = 0.0;
            color.z = 0.0;
        }
    }

    return color;
}

struct FragmentOutput {
    @builtin(frag_depth) depth: f32,
    @location(0) color: vec4<f32>,
}

const NEAR = 0.0005;

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    let t = -in.near.y / (in.far.y - in.near.y);
    let pos = in.near + t * (in.far - in.near);
    let color = (grid(pos, 1.0, false) + grid(pos, 0.1, true)) * f32(t > 0.0);
    let clip = view.view_proj * vec4<f32>(pos.xyz, 1.0);
    var out = FragmentOutput();
    out.depth = clip.z / clip.w;
    out.color = color * (1.0 - NEAR / out.depth);
    return out;
}
