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
    // previous image
    let color1 = textureSample(tex1, samp, in.uv);
    // current image
    let color2 = textureSample(tex2, samp, in.uv);

    let progress = u_params.progress;

    // crossfade transition (anim_type == 0)
    if u_params.anim_type == 0u {
        return mix(color1, color2, progress);
    } else if u_params.anim_type == 1u {
        // Sweep with soft edge
        let sweep_dir = u_params.direction;

        // Calculate position along sweep direction
        let coord = dot(in.uv, sweep_dir);

        // Normalize to [0, 1] range by dividing by the max possible coordinate
        // Max coordinate is at corner [1, 1]
        let max_coord = dot(vec2<f32>(1.0, 1.0), sweep_dir);
        let normalized_coord = coord / max_coord;

        let edge_width = 0.05;
        // Map progress from [0, 1] to [-edge_width, 1 + edge_width]
        let sweep_position = u_params.progress * (1.0 + 2.0 * edge_width) - edge_width;

        let blend_factor = smoothstep(sweep_position - edge_width, sweep_position + edge_width, normalized_coord);

        return mix(color2, color1, blend_factor);
    }

    // TODO: seperate transitions into different files

    // fallback: show current image if no animation or invalid
    return color2;
}
