#include <metal_stdlib>
using namespace metal;

struct Instance {
    float2 origin;
    float2 size;
    float4 color;
    float2 uv_origin;
    float2 uv_size;
    uint4 metadata;
};

struct VertexOut {
    float4 position [[position]];
    float2 uv;
    float4 color;
    uint kind [[flat]];
};

float4 unpack_rgba(uint color) {
    return float4(
        float((color >> 24) & 0xff) / 255.0,
        float((color >> 16) & 0xff) / 255.0,
        float((color >> 8) & 0xff) / 255.0,
        float(color & 0xff) / 255.0
    );
}

float linearize_component(float value) {
    return value <= 0.04045
        ? value / 12.92
        : pow((value + 0.055) / 1.055, 2.4);
}

float relative_luminance(float3 color) {
    return 0.2126 * linearize_component(color.r)
        + 0.7152 * linearize_component(color.g)
        + 0.0722 * linearize_component(color.b);
}

float contrast_ratio(float3 first, float3 second) {
    float first_luminance = relative_luminance(first);
    float second_luminance = relative_luminance(second);
    return (max(first_luminance, second_luminance) + 0.05)
        / (min(first_luminance, second_luminance) + 0.05);
}

float4 contrasted_color(float minimum_contrast, float4 foreground, float4 background) {
    if (contrast_ratio(foreground.rgb, background.rgb) >= minimum_contrast) {
        return foreground;
    }
    float white_contrast = contrast_ratio(float3(1.0), background.rgb);
    float black_contrast = contrast_ratio(float3(0.0), background.rgb);
    return white_contrast > black_contrast
        ? float4(1.0, 1.0, 1.0, foreground.a)
        : float4(0.0, 0.0, 0.0, foreground.a);
}

vertex VertexOut terminal_vertex(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    constant Instance *instances [[buffer(0)]],
    constant float2 &viewport [[buffer(1)]])
{
    constexpr float2 unit[6] = {
        float2(0.0, 0.0), float2(1.0, 0.0), float2(0.0, 1.0),
        float2(1.0, 0.0), float2(1.0, 1.0), float2(0.0, 1.0)
    };
    Instance instance = instances[instance_id];
    float2 local = unit[vertex_id];
    float2 pixel = instance.origin + local * instance.size;
    VertexOut out;
    out.position = float4(
        pixel.x / viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / viewport.y * 2.0,
        0.0,
        1.0);
    out.uv = instance.uv_origin + local * instance.uv_size;
    out.color = instance.color;
    out.kind = instance.metadata.x;
    float minimum_contrast = as_type<float>(instance.metadata.z);
    if (out.kind == 1 && minimum_contrast > 1.0 && (instance.metadata.w & 1u) == 0) {
        out.color = contrasted_color(
            minimum_contrast,
            out.color,
            unpack_rgba(instance.metadata.y));
    }
    return out;
}

fragment float4 terminal_fragment(
    VertexOut in [[stage_in]],
    texture2d<float> mask_atlas [[texture(0)]],
    texture2d<float> color_atlas [[texture(1)]],
    sampler atlas_sampler [[sampler(0)]])
{
    if (in.kind == 0) {
        return in.color;
    }
    if (in.kind == 1) {
        float2 size = float2(mask_atlas.get_width(), mask_atlas.get_height());
        float coverage = mask_atlas.sample(atlas_sampler, in.uv / size).r;
        return float4(in.color.rgb, in.color.a * coverage);
    }

    // CoreGraphics produces premultiplied RGBA. The render pipeline uses source-alpha blending,
    // so restore straight RGB here to avoid dark halos around antialiased color glyph edges.
    float2 size = float2(color_atlas.get_width(), color_atlas.get_height());
    float4 glyph = color_atlas.sample(atlas_sampler, in.uv / size);
    float3 straight_rgb = glyph.a > 0.0 ? glyph.rgb / glyph.a : float3(0.0);
    return float4(straight_rgb, glyph.a * in.color.a);
}

fragment float4 terminal_image_fragment(
    VertexOut in [[stage_in]],
    texture2d<float> image [[texture(0)]],
    sampler image_sampler [[sampler(0)]])
{
    float4 color = image.sample(image_sampler, in.uv);
    return float4(color.rgb, color.a * in.color.a);
}
