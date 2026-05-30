const FIT_STRETCH: u32 = 0u;
const FIT_CONTAIN: u32 = 1u;
const FIT_COVER: u32 = 2u;
const FIT_CENTER: u32 = 3u;
const FIT_TILE: u32 = 4u;

fn inside_unit(uv: vec2<f32>) -> bool {
    return all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
}

fn centered_uv(screen_uv: vec2<f32>, scale: vec2<f32>) -> vec2<f32> {
    return (screen_uv - vec2<f32>(0.5)) / scale + vec2<f32>(0.5);
}

fn stretch_uv(screen_uv: vec2<f32>) -> vec2<f32> {
    return screen_uv;
}

fn contain_uv(
    screen_uv: vec2<f32>,
    image_size: vec2<f32>,
    output_size: vec2<f32>,
) -> vec2<f32> {
    let image_aspect = image_size.x / image_size.y;
    let output_aspect = output_size.x / output_size.y;

    var scale = vec2<f32>(1.0);

    if image_aspect > output_aspect {
        // Image is wider: full width, bars top/bottom.
        scale.y = output_aspect / image_aspect;
    } else {
        // Image is taller: full height, bars left/right.
        scale.x = image_aspect / output_aspect;
    }

    return centered_uv(screen_uv, scale);
}

fn cover_uv(
    screen_uv: vec2<f32>,
    image_size: vec2<f32>,
    output_size: vec2<f32>,
) -> vec2<f32> {
    let image_aspect = image_size.x / image_size.y;
    let output_aspect = output_size.x / output_size.y;

    var scale = vec2<f32>(1.0);

    if image_aspect > output_aspect {
        // Image is wider: crop left/right.
        scale.x = image_aspect / output_aspect;
    } else {
        // Image is taller: crop top/bottom.
        scale.y = output_aspect / image_aspect;
    }

    return centered_uv(screen_uv, scale);
}

fn center_uv(
    screen_uv: vec2<f32>,
    image_size: vec2<f32>,
    output_size: vec2<f32>,
) -> vec2<f32> {
    let scale = image_size / output_size;
    return centered_uv(screen_uv, scale);
}

fn tile_uv(
    screen_uv: vec2<f32>,
    image_size: vec2<f32>,
    output_size: vec2<f32>,
) -> vec2<f32> {
    return fract(screen_uv * (output_size / image_size));
}

fn map_uv(
    screen_uv: vec2<f32>,
    image_size: vec2<f32>,
    output_size: vec2<f32>,
    fit_mode: u32,
) -> vec2<f32> {
    if fit_mode == FIT_STRETCH {
        return stretch_uv(screen_uv);
    }

    if fit_mode == FIT_CONTAIN {
        return contain_uv(screen_uv, image_size, output_size);
    }

    if fit_mode == FIT_COVER {
        return cover_uv(screen_uv, image_size, output_size);
    }

    if fit_mode == FIT_CENTER {
        return center_uv(screen_uv, image_size, output_size);
    }

    if fit_mode == FIT_TILE {
        return tile_uv(screen_uv, image_size, output_size);
    }

    return cover_uv(screen_uv, image_size, output_size);
}

fn should_use_background(fit_mode: u32, uv: vec2<f32>) -> bool {
    if fit_mode == FIT_CONTAIN || fit_mode == FIT_CENTER {
        return !inside_unit(uv);
    }

    return false;
}

fn sample_sized(
    tex: texture_2d<f32>,
    samp: sampler,
    screen_uv: vec2<f32>,
    output_size: vec2<f32>,
    fit_mode: u32,
    background: vec4<f32>,
) -> vec4<f32> {
    let dims = textureDimensions(tex);
    let image_size = vec2<f32>(f32(dims.x), f32(dims.y));
    let uv = map_uv(screen_uv, image_size, output_size, fit_mode);

    if should_use_background(fit_mode, uv) {
        return background;
    }

    return textureSample(tex, samp, uv);
}
