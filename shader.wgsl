@group(0) @binding(0)
var samp: sampler;
@group(0) @binding(1)
var tex: texture_2d<f32>;

struct VertexInput {
    @location(0) pos: vec2f,
    @location(1) uv: vec2f,
};

struct VertexOutput {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f
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
    return textureSample(tex, samp, in.uv);
}