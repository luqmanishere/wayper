@group(0) @binding(0)
var samp: sampler;
@group(0) @binding(1)
var tex1: texture_2d<f32>;
@group(0) @binding(2)
var tex2: texture_2d<f32>;
@group(0) @binding(3)
var<uniform> u_params: TransitionParams;

struct VertexInput {
    @location(0) pos: vec2f,
    @location(1) uv: vec2f,
};

struct VertexOutput {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f
}

struct TransitionParams {
    progress: f32,
    anim_type: u32,
    direction: vec2<f32>
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.pos = vec4f(in.pos, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let color1 = textureSample(tex1, samp, in.uv);
    let color2 = textureSample(tex2, samp, in.uv);
    return textureSample(tex1, samp, in.uv);
}