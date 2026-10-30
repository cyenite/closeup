struct Params {
    dest: vec4<f32>,
    source: vec4<f32>,
    texel: vec2<f32>,
    mode: u32,
    opacity: f32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var linear: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

const CORNERS = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

const BACKDROP = vec4<f32>(0.07, 0.07, 0.08, 1.0);

@vertex
fn vs(@builtin(vertex_index) index: u32) -> Varyings {
    let corner = CORNERS[index];
    let output = params.dest.xy + corner * params.dest.zw;
    var out: Varyings;
    out.position = vec4(output.x * 2.0 - 1.0, 1.0 - output.y * 2.0, 0.0, 1.0);
    out.uv = params.source.xy + corner * params.source.zw;
    return out;
}

// Catmull-Rom bicubic in 9 bilinear taps: keeps zoomed text crisp where
// plain bilinear would smear it.
fn sample_bicubic(uv: vec2<f32>) -> vec4<f32> {
    let size = 1.0 / params.texel;
    let position = uv * size;
    let center = floor(position - 0.5) + 0.5;
    let f = position - center;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let offset12 = w2 / w12;
    let t0 = (center - 1.0) * params.texel;
    let t3 = (center + 2.0) * params.texel;
    let t12 = (center + offset12) * params.texel;
    var color = vec4<f32>(0.0);
    color += textureSampleLevel(image, linear, vec2(t0.x, t0.y), 0.0) * w0.x * w0.y;
    color += textureSampleLevel(image, linear, vec2(t12.x, t0.y), 0.0) * w12.x * w0.y;
    color += textureSampleLevel(image, linear, vec2(t3.x, t0.y), 0.0) * w3.x * w0.y;
    color += textureSampleLevel(image, linear, vec2(t0.x, t12.y), 0.0) * w0.x * w12.y;
    color += textureSampleLevel(image, linear, vec2(t12.x, t12.y), 0.0) * w12.x * w12.y;
    color += textureSampleLevel(image, linear, vec2(t3.x, t12.y), 0.0) * w3.x * w12.y;
    color += textureSampleLevel(image, linear, vec2(t0.x, t3.y), 0.0) * w0.x * w3.y;
    color += textureSampleLevel(image, linear, vec2(t12.x, t3.y), 0.0) * w12.x * w3.y;
    color += textureSampleLevel(image, linear, vec2(t3.x, t3.y), 0.0) * w3.x * w3.y;
    return color;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    if (params.mode == 0u) {
        if (any(in.uv < vec2(0.0)) || any(in.uv > vec2(1.0))) {
            return BACKDROP;
        }
        return vec4(clamp(sample_bicubic(in.uv).rgb, vec3(0.0), vec3(1.0)), 1.0);
    }
    if (params.mode == 1u) {
        return textureSampleLevel(image, linear, in.uv, 0.0) * params.opacity;
    }
    return BACKDROP;
}
