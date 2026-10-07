// WGC FP16 is linear scRGB: RGB primaries are Rec.709, nominal white is 80 nits.
Texture2D<float4> capture : register(t0);
cbuffer DisplayColor : register(b0) { float white; float hdr; float2 padding; };

float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 position = float2((id << 1) & 2, id & 2);
    return float4(position * float2(2, -2) + float2(-1, 1), 0, 1);
}
float3 srgb(float3 value) {
    return float3(
        value.r <= 0.0031308 ? 12.92 * value.r : 1.055 * pow(value.r, 1.0 / 2.4) - 0.055,
        value.g <= 0.0031308 ? 12.92 * value.g : 1.055 * pow(value.g, 1.0 / 2.4) - 0.055,
        value.b <= 0.0031308 ? 12.92 * value.b : 1.055 * pow(value.b, 1.0 / 2.4) - 0.055);
}
float4 pixel(float4 position : SV_Position) : SV_Target {
    float3 rgb = max(capture.Load(int3(position.xy, 0)).rgb, 0) / max(white, 1);
    // A fixed shoulder keeps text/midtones stable, and avoids exposure pumping.
    // Compress by peak to preserve RGB ratios before clamping to the SDR gamut.
    float peak = max(rgb.r, max(rgb.g, rgb.b));
    if (hdr > 0 && peak > 0.8) {
        float mapped = 0.8 + 0.2 * (1 - exp(-(peak - 0.8) / 0.2));
        rgb *= mapped / peak;
    }
    return float4(srgb(saturate(rgb)), 1);
}
