#version 300 es
precision highp float;

in vec2 v_uv;
out vec4 fragColor;

uniform float u_time;
uniform vec2 u_resolution;

vec3 mod289(vec3 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec2 mod289(vec2 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec3 permute(vec3 x) { return mod289(((x * 34.0) + 1.0) * x); }

float snoise(vec2 v) {
    const vec4 C = vec4(
        0.211324865405187,
        0.366025403784439,
       -0.577350269189626,
        0.024390243902439
    );
    vec2 i = floor(v + dot(v, C.yy));
    vec2 x0 = v - i + dot(i, C.xx);
    vec2 i1 = (x0.x > x0.y) ? vec2(1.0, 0.0) : vec2(0.0, 1.0);
    vec4 x12 = x0.xyxy + C.xxzz;
    x12.xy -= i1;
    i = mod289(i);
    vec3 p = permute(permute(i.y + vec3(0.0, i1.y, 1.0)) + i.x + vec3(0.0, i1.x, 1.0));
    vec3 m = max(0.5 - vec3(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), 0.0);
    m = m * m;
    m = m * m;
    vec3 x = 2.0 * fract(p * C.www) - 1.0;
    vec3 h = abs(x) - 0.5;
    vec3 ox = floor(x + 0.5);
    vec3 a0 = x - ox;
    m *= 1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h);
    vec3 g;
    g.x = a0.x * x0.x + h.x * x0.y;
    g.yz = a0.yz * x12.xz + h.yz * x12.yw;
    return 130.0 * dot(m, g);
}

float fbm(vec2 p) {
    float value = 0.0;
    float amplitude = 0.5;
    for (int i = 0; i < 5; i++) {
        value += amplitude * snoise(p);
        p *= 2.0;
        amplitude *= 0.5;
    }
    return value;
}

void main() {
    vec2 uv = v_uv;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = (uv - 0.5) * aspect;

    float t = u_time * 0.3;
    vec2 drift = vec2(0.0, -t);

    float n1 = fbm(p * 3.0 + drift);
    float n2 = fbm(p * 5.0 + drift * 1.3 + 3.14);
    float n3 = fbm(p * 2.0 + drift * 0.7 + 6.28);

    float fire = n1 * 0.5 + n2 * 0.3 + n3 * 0.2;

    float vignette = 1.0 - length(p * vec2(1.2, 0.8));
    vignette = smoothstep(0.0, 0.7, vignette);

    float rise = smoothstep(-0.1, 0.4, -p.y + fire * 0.3);

    float intensity = fire * vignette * rise;
    intensity = smoothstep(0.0, 0.6, intensity);

    vec3 col1 = vec3(0.6, 0.1, 0.0);
    vec3 col2 = vec3(0.976, 0.451, 0.086);
    vec3 col3 = vec3(0.984, 0.749, 0.141);

    vec3 color = mix(col1, col2, smoothstep(0.0, 0.5, intensity));
    color = mix(color, col3, smoothstep(0.5, 1.0, intensity));

    // Composite fire onto black background directly
    // (avoids alpha blending issues with CSS-composited canvas)
    vec3 final_color = color * intensity * 1.2;

    fragColor = vec4(final_color, 1.0);
}
