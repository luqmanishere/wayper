@group(0) @binding(0)
var samp: sampler;

@group(0) @binding(1)
var tex: texture_2d<f32>;

@group(0) @binding(2)
var<uniform> node: ImageNodeParams;

struct ImageNodeParams {
       rect: vec4<f32>,
       output_size: vec2<f32>,
       opacity: f32,
       fit_mode: u32,
       background: vec4<f32>,
}

struct VertexInput {
       @location(0) local_pos: vec2<f32>,
       @location(1) uv: vec2<f32>,
}

struct VertexOutput {
       @builtin(position) pos: vec4<f32>,
       @location(0) uv: vec2<f32>,
}

fn pixel_to_clip(pos: vec2<f32>, output_size: vec2<f32>) -> vec2<f32> {
   let normalized = pos / output_size;
   return vec2<f32>(
   	  normalized.x * 2.0 - 1.0,
	  1.0 - normalized.y * 2.0,
   );
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
   var out: VertexOutput;

   let pixel_pos = node.rect.xy + in.local_pos * node.rect.zw;

   out.pos = vec4<f32>(
   	   pixel_to_clip(pixel_pos, node.output_size),
	   0.0,
	   1.0,
   );

   out.uv = in.uv;
   return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
   let color = sample_sized(
       tex,
       samp,
       in.uv,
       node.rect.zw,
       node.fit_mode,
       node.background,
   );
   return vec4<f32>(color.rgb, color.a * node.opacity);
}