struct EffectUniforms {
    tex_size: vec2<f32>,
    tex_diff_u: vec2<f32>,
    effect_params: vec4<f32>,
};

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(0) @binding(0) var pal_texture: texture_2d<f32>;
@group(0) @binding(1) var pal_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: EffectUniforms;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.tex_coord = input.tex_coord;
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let offset = vec2<f32>(uniforms.effect_params.x, uniforms.effect_params.y);
    let level = max(uniforms.effect_params.z, 0.0);
    let center = vec2<f32>(0.5, 0.5) + offset;
    let direction = input.tex_coord - center;
    let strength = level * 0.015625;

    var color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    let sample_count = 12.0;
    for (var i: i32 = 0; i < 12; i = i + 1) {
        let t = f32(i) / (sample_count - 1.0);
        let uv = input.tex_coord - direction * strength * t;
        color = color + textureSample(pal_texture, pal_sampler, uv);
    }
    return (color / sample_count) * input.color;
}
