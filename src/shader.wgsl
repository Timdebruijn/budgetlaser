struct Uniforms {
    res: vec2<f32>,         // internal render target size in px
    half_width: f32,        // line half width in px
    decay: f32,             // glow decay per frame (0 = off)
    exposure: f32,
    gamma_out: f32,         // 1 = apply gamma in shader (non-sRGB surface)
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

// ---- line pass: one capsule per instance -----------------------------------

struct Seg {
    @location(0) p0: vec2<f32>,
    @location(1) p1: vec2<f32>,
    @location(2) rgb: vec3<f32>,
};

struct LineOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) px: vec2<f32>,
    @location(1) a: vec2<f32>,
    @location(2) b: vec2<f32>,
    @location(3) rgb: vec3<f32>,
};

// Segments arrive in NDC of the whole target (zone mapping happens on the CPU).
fn to_px(p: vec2<f32>) -> vec2<f32> {
    return (p * 0.5 + 0.5) * u.res;
}

@vertex
fn vs_line(@builtin(vertex_index) vi: u32, s: Seg) -> LineOut {
    let a = to_px(s.p0);
    let b = to_px(s.p1);
    var d = b - a;
    if (dot(d, d) < 1e-6) { d = vec2<f32>(1.0, 0.0); } else { d = normalize(d); }
    let n = vec2<f32>(-d.y, d.x);
    let r = u.half_width + 1.5;
    // quad corners: 0 = a-, 1 = a+, 2 = b-, 3 = b+; triangles (0,1,2) (2,1,3)
    let corner = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u)[vi];
    let along = select(a - d * r, b + d * r, corner >= 2u);
    let side = select(-1.0, 1.0, (corner & 1u) == 1u);
    let px = along + n * side * r;
    var o: LineOut;
    o.pos = vec4<f32>(px / u.res * 2.0 - 1.0, 0.0, 1.0);
    o.px = px;
    o.a = a;
    o.b = b;
    o.rgb = s.rgb;
    return o;
}

@fragment
fn fs_line(i: LineOut) -> @location(0) vec4<f32> {
    let ab = i.b - i.a;
    let t = clamp(dot(i.px - i.a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    let dist = length(i.px - (i.a + ab * t));
    let cov = 1.0 - smoothstep(u.half_width - 0.75, u.half_width + 0.75, dist);
    return vec4<f32>(i.rgb * cov, 1.0);
}

// ---- fullscreen passes: decay (previous frame) and tonemap -----------------

@group(1) @binding(0) var src: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct FsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> FsOut {
    let xy = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u)); // (0,0) (2,0) (0,2)
    var o: FsOut;
    o.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return o;
}

@fragment
fn fs_decay(i: FsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(src, samp, i.uv).rgb * u.decay, 1.0);
}

@fragment
fn fs_tonemap(i: FsOut) -> @location(0) vec4<f32> {
    let hdr = max(textureSample(src, samp, i.uv).rgb * u.exposure, vec3<f32>(0.0));
    var c = 1.0 - exp(-hdr);               // soft shoulder, black stays exactly black
    if (u.gamma_out > 0.5) { c = pow(c, vec3<f32>(1.0 / 2.2)); }
    return vec4<f32>(c, 1.0);
}
