const CUBE: array<vec3<f32>, 18> = array<vec3<f32>, 18>(
    vec3<f32>(-1.0, -1.0,  1.0), vec3<f32>( 1.0, -1.0,  1.0), vec3<f32>( 1.0,  1.0,  1.0),
    vec3<f32>(-1.0, -1.0,  1.0), vec3<f32>( 1.0,  1.0,  1.0), vec3<f32>(-1.0,  1.0,  1.0),
    vec3<f32>(-1.0,  1.0,  1.0), vec3<f32>( 1.0,  1.0,  1.0), vec3<f32>( 1.0,  1.0, -1.0),
    vec3<f32>(-1.0,  1.0,  1.0), vec3<f32>( 1.0,  1.0, -1.0), vec3<f32>(-1.0,  1.0, -1.0),
    vec3<f32>( 1.0, -1.0,  1.0), vec3<f32>( 1.0, -1.0, -1.0), vec3<f32>( 1.0,  1.0, -1.0),
    vec3<f32>( 1.0, -1.0,  1.0), vec3<f32>( 1.0,  1.0, -1.0), vec3<f32>( 1.0,  1.0,  1.0)
);

struct Parameters {
    values: vec4<u32>,
};

@group(0) @binding(0) var<uniform> parameters: Parameters;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let count = max(f32(parameters.values.x), 1.0);
    let columns = max(u32(ceil(sqrt(count))), 1u);
    let rows = max(u32(ceil(count / f32(columns))), 1u);
    let column = instance_index % columns;
    let row = instance_index / columns;
    let cell_x = 1.75 / f32(columns);
    let cell_y = 1.75 / f32(rows);
    let center = vec2<f32>(
        -0.875 + (f32(column) + 0.5) * cell_x,
         0.875 - (f32(row) + 0.5) * cell_y
    );
    let vertex = CUBE[vertex_index];
    let projected = vec2<f32>(vertex.x + vertex.z * 0.55, vertex.y + vertex.z * 0.35);
    let screen = center + projected * vec2<f32>(cell_x, cell_y) * 0.38;
    let face = vertex_index / 6u;
    var color = vec3<f32>(0.25, 0.62, 0.92);
    if face == 1u {
        color = vec3<f32>(0.18, 0.42, 0.72);
    }
    if face == 2u {
        color = vec3<f32>(0.48, 0.77, 0.98);
    }
    let shade = 0.72 + f32(instance_index % 7u) * 0.04;
    var output: VertexOutput;
    output.position = vec4<f32>(screen, 0.5, 1.0);
    output.color = vec4<f32>(color * shade, 1.0);
    return output;
}

@fragment
fn fs_main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}
