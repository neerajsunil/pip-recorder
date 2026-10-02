// ABI-safe bridge: Intel's own headers define all runtime structures. Encoding
// uses bounded system-memory NV12 surfaces; the Rust backend owns GPU readback.
#include <windows.h>
#include <vpl/mfxvideo.h>
#include <vector>
#include <memory>
#include <cstring>
#include <cstdio>
#include <thread>
#include <chrono>
struct Intel {
    HMODULE dll = nullptr;
    mfxSession session = nullptr;
    decltype(&MFXInitEx) init = nullptr;
    decltype(&MFXInitialize) initialize = nullptr;
    decltype(&MFXClose) close;
    decltype(&MFXVideoCORE_SetHandle) set_handle;
    decltype(&MFXVideoCORE_SyncOperation) sync;
    decltype(&MFXVideoENCODE_Query) query;
    decltype(&MFXVideoENCODE_Init) start;
    decltype(&MFXVideoENCODE_Close) end;
    decltype(&MFXVideoENCODE_EncodeFrameAsync) encode;
    mfxVideoParam param{};
    std::vector<mfxFrameSurface1> surfaces;
    std::vector<std::vector<unsigned char>> pixels;
    std::vector<unsigned char> output;
    mfxBitstream bitstream{};
    mfxExtAV1BitstreamParam av1{};
    mfxExtBuffer* extensions[1]{};
    bool started = false;
    ~Intel() { if (session) { if (started) end(session); close(session); } if (dll) FreeLibrary(dll); }
};
static bool bind(Intel& p) {
#define RESOLVE(field, name) p.field = reinterpret_cast<decltype(p.field)>(GetProcAddress(p.dll, #name)); if (!p.field) return false;
    p.init = reinterpret_cast<decltype(p.init)>(GetProcAddress(p.dll, "MFXInitEx"));
    p.initialize = reinterpret_cast<decltype(p.initialize)>(GetProcAddress(p.dll, "MFXInitialize"));
    if (!p.init && !p.initialize) return false;
    RESOLVE(close, MFXClose)
    RESOLVE(set_handle, MFXVideoCORE_SetHandle) RESOLVE(sync, MFXVideoCORE_SyncOperation)
    RESOLVE(query, MFXVideoENCODE_Query) RESOLVE(start, MFXVideoENCODE_Init)
    RESOLVE(end, MFXVideoENCODE_Close) RESOLVE(encode, MFXVideoENCODE_EncodeFrameAsync)
#undef RESOLVE
    return true;
}
static bool load(Intel& p, bool legacy_only = false) {
    for (auto name : {L"libmfx64-gen.dll", L"libmfxhw64.dll"}) {
        if (legacy_only && wcscmp(name, L"libmfxhw64.dll") != 0) continue;
        p.dll = LoadLibraryExW(name, nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!p.dll) continue;
        if (bind(p)) return true;
        FreeLibrary(p.dll); p.dll = nullptr;
    }
    return false;
}
extern "C" void* fr_intel_open(void* device, unsigned codec, unsigned width, unsigned height,
    unsigned fps, unsigned bitrate, unsigned gop, int cbr, int probe, char* error, unsigned capacity) try {
    auto p = std::make_unique<Intel>();
    auto fail = [&](const char* stage, int status) -> void* { snprintf(error, capacity, "%s (%d)", stage, status); return nullptr; };
    if (!load(*p)) return fail("Intel hardware runtime not installed or incompatible", -1);
    mfxInitParam init{};
    init.Implementation = MFX_IMPL_HARDWARE_ANY | MFX_IMPL_VIA_D3D11;
    init.Version.Major = 1; init.Version.Minor = 0;
    mfxStatus status;
    if (p->initialize) { mfxInitializationParam modern{}; modern.AccelerationMode = MFX_ACCEL_MODE_VIA_D3D11; status = p->initialize(modern, &p->session); }
    else { status = p->init(init, &p->session); }
    if (status < 0 && p->initialize) {
        if (p->session) p->close(p->session);
        p->session = nullptr; FreeLibrary(p->dll); p->dll = nullptr;
        if (!load(*p, true) || !p->init) return fail("Modern Intel runtime failed and legacy runtime is unavailable", status);
        status = p->init(init, &p->session);
    }
    if (status < 0) return fail("Intel session initialization failed", status);
    status = p->set_handle(p->session, MFX_HANDLE_D3D11_DEVICE, device);
    if (status < 0) return fail("Intel runtime rejected the selected D3D11 adapter", status);
    auto& v = p->param;
    if (codec == 0) { p->av1.Header.BufferId = MFX_EXTBUFF_AV1_BITSTREAM_PARAM; p->av1.Header.BufferSz = sizeof(p->av1); p->av1.WriteIVFHeaders = MFX_CODINGOPTION_OFF; p->extensions[0] = &p->av1.Header; v.ExtParam = p->extensions; v.NumExtParam = 1; }
    v.AsyncDepth = 1; v.IOPattern = MFX_IOPATTERN_IN_SYSTEM_MEMORY;
    v.mfx.CodecId = codec == 0 ? MFX_CODEC_AV1 : codec == 1 ? MFX_CODEC_HEVC : MFX_CODEC_AVC;
    v.mfx.TargetUsage = MFX_TARGETUSAGE_BEST_QUALITY;
    v.mfx.BRCParamMultiplier = static_cast<mfxU16>((bitrate / 1000 + 65534) / 65535);
    v.mfx.TargetKbps = static_cast<mfxU16>(bitrate / 1000 / v.mfx.BRCParamMultiplier);
    v.mfx.RateControlMethod = cbr ? MFX_RATECONTROL_CBR : MFX_RATECONTROL_VBR;
    v.mfx.GopPicSize = static_cast<mfxU16>(fps * gop); v.mfx.GopRefDist = 1; v.mfx.NumRefFrame = 1;
    auto& f = v.mfx.FrameInfo;
    f.FourCC = MFX_FOURCC_NV12; f.ChromaFormat = MFX_CHROMAFORMAT_YUV420;
    f.PicStruct = MFX_PICSTRUCT_PROGRESSIVE; f.Width = static_cast<mfxU16>((width + 31) & ~31);
    f.Height = static_cast<mfxU16>((height + 15) & ~15);
    f.CropW = static_cast<mfxU16>(width); f.CropH = static_cast<mfxU16>(height);
    f.FrameRateExtN = fps; f.FrameRateExtD = 1;
    mfxVideoParam checked = v;
    status = p->query(p->session, &v, &checked);
    if (status < 0) return fail("Codec unavailable in this Intel GPU/runtime", status);
    // Partial acceleration must not be presented as hardware encoding.
    if (status == MFX_WRN_PARTIAL_ACCELERATION) return fail("Intel reported partial software acceleration", status);
    if (checked.mfx.CodecId != v.mfx.CodecId || checked.mfx.FrameInfo.FourCC != MFX_FOURCC_NV12) return fail("Intel changed the requested codec / input format", -1);
    if (checked.mfx.GopRefDist > 1) return fail("Intel enabled unsupported frame reordering", -1);
    v = checked;
    if (probe) return p.release();
    status = p->start(p->session, &v);
    if (status < 0 || status == MFX_WRN_PARTIAL_ACCELERATION) return fail("Intel encoder initialization failed", status);
    p->started = true;
    p->surfaces.resize(8); p->pixels.resize(8);
    for (size_t i = 0; i < p->surfaces.size(); ++i) {
        auto& surface = p->surfaces[i]; surface.Info = v.mfx.FrameInfo;
        auto pitch = surface.Info.Width; auto aligned_h = surface.Info.Height;
        p->pixels[i].resize(static_cast<size_t>(pitch) * aligned_h * 3 / 2);
        surface.Data.Pitch = pitch; surface.Data.Y = p->pixels[i].data();
        surface.Data.UV = p->pixels[i].data() + static_cast<size_t>(pitch) * aligned_h;
    }
    p->output.resize(static_cast<size_t>(width) * height * 3 / 2 + 1024 * 1024);
    return p.release();
} catch (const std::exception& exception) { snprintf(error, capacity, "Intel initialization: %s", exception.what()); return nullptr; }
catch (...) { snprintf(error, capacity, "Intel initialization failed"); return nullptr; }
extern "C" int fr_intel_encode(void* handle, const unsigned char* nv12, unsigned width, unsigned height,
    unsigned long long index, const unsigned char** data, unsigned* length, unsigned long long* timestamp, int* key) {
    auto& p = *static_cast<Intel*>(handle);
    mfxFrameSurface1* surface = nullptr;
    if (nv12) {
        for (auto& candidate : p.surfaces) if (!candidate.Data.Locked) { surface = &candidate; break; }
        if (!surface) return MFX_ERR_MEMORY_ALLOC;
        auto pitch = surface->Data.Pitch;
        for (unsigned row = 0; row < height; ++row) memcpy(surface->Data.Y + row * pitch, nv12 + row * width, width);
        for (unsigned row = 0; row < height / 2; ++row) memcpy(surface->Data.UV + row * pitch, nv12 + width * height + row * width, width);
        surface->Data.TimeStamp = index * 90000 / p.param.mfx.FrameInfo.FrameRateExtN;
        surface->Data.FrameOrder = static_cast<mfxU32>(index);
    }
    p.bitstream = {};
    p.bitstream.Data = p.output.data(); p.bitstream.MaxLength = static_cast<mfxU32>(p.output.size());
    mfxSyncPoint point = nullptr;
    mfxStatus status;
    unsigned retries = 0;
    do {
        status = p.encode(p.session, nullptr, surface, &p.bitstream, &point);
        if (status == MFX_WRN_DEVICE_BUSY) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    } while (status == MFX_WRN_DEVICE_BUSY && ++retries < 2000);
    if (status == MFX_ERR_MORE_DATA) return 1;
    if (status < 0 || !point) return status < 0 ? status : MFX_ERR_UNKNOWN;
    status = p.sync(p.session, point, 5000);
    if (status != MFX_ERR_NONE) return status;
    *data = p.bitstream.Data + p.bitstream.DataOffset; *length = p.bitstream.DataLength;
    *timestamp = p.bitstream.TimeStamp;
    *key = (p.bitstream.FrameType & (MFX_FRAMETYPE_IDR | MFX_FRAMETYPE_I)) != 0;
    return 0;
}
extern "C" void fr_intel_close(void* handle) { delete static_cast<Intel*>(handle); }
