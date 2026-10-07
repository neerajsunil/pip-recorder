//! Encoder/GPU choice, capability inventory and the derived labels.
use super::{
    notify::notify_issue,
    settings::persist_preferences,
    state::{AppState, Lock, Shared},
};
use crate::MainWindow;
use fastrecorder_core::{Codec, EncoderPreference, recommended_bitrate_mbps};
use fastrecorder_platform as native;
use slint::ComponentHandle;

pub(super) fn preference(choice: i32) -> EncoderPreference {
    match choice {
        1 => EncoderPreference::NvencAv1,
        2 => EncoderPreference::NvencHevc,
        3 => EncoderPreference::NvencH264,
        4 => EncoderPreference::IntelAv1,
        5 => EncoderPreference::IntelHevc,
        6 => EncoderPreference::IntelH264,
        7 => EncoderPreference::SoftwareOnly,
        8 => EncoderPreference::SoftwareAv1,
        _ => EncoderPreference::Auto,
    }
}
pub(super) fn selected_gpu(ui: &MainWindow, state: &AppState) -> Option<u32> {
    if ui.get_gpu_choice() > 0 {
        return state
            .gpus
            .get((ui.get_gpu_choice() - 1) as usize)
            .map(|gpu| gpu.index);
    }
    let choice = ui.get_encoder_choice();
    if (1..=6).contains(&choice) {
        let vendor = if choice <= 3 { 0x10de } else { 0x8086 };
        let codec = ((choice - 1) % 3) as usize;
        return state
            .gpus
            .iter()
            .find(|gpu| gpu.vendor == vendor && gpu.codecs[codec])
            .or_else(|| state.gpus.iter().find(|gpu| gpu.vendor == vendor))
            .map(|gpu| gpu.index);
    }
    state
        .gpus
        .iter()
        .find(|gpu| gpu.vendor == 0x10de && gpu.av1)
        .or_else(|| state.gpus.iter().find(|gpu| gpu.av1))
        .or_else(|| state.gpus.iter().find(|gpu| gpu.codecs[1]))
        .or_else(|| state.gpus.iter().find(|gpu| gpu.codecs[2]))
        .map(|gpu| gpu.index)
}
pub(super) const ENCODER_NAMES: [&str; 9] = [
    "Automatic · best available",
    "NVIDIA NVENC · AV1",
    "NVIDIA NVENC · HEVC",
    "NVIDIA NVENC · H.264",
    "Intel Quick Sync · AV1",
    "Intel Quick Sync · HEVC",
    "Intel Quick Sync · H.264",
    "Windows software · H.264",
    "rav1e software · AV1",
];
/// Format choices in Advanced, as (label, codec). The codec numbers match
/// `video-codec`: 0 automatic, 1 AV1, 2 HEVC, 3 H.264.
const CODEC_NAMES: [&str; 4] = [
    "Automatic · smallest",
    "AV1",
    "HEVC · H.265",
    "H.264 · plays everywhere",
];

/// The format an encoder choice produces (0 when Pip decides).
fn codec_of(choice: i32, prefer_h264: bool) -> usize {
    match choice {
        0 if prefer_h264 => 3,
        0 => 0,
        1 | 4 | 8 => 1,
        2 | 5 => 2,
        _ => 3,
    }
}

/// Encoders on this PC that can make `codec`, as (label, encoder choice).
fn encoders_for(codec: usize, available: &[usize]) -> Vec<(&'static str, i32)> {
    let options: &[(&str, i32)] = match codec {
        1 => &[
            ("NVIDIA NVENC", 1),
            ("Intel Quick Sync", 4),
            ("Software · rav1e (CPU)", 8),
        ],
        2 => &[("NVIDIA NVENC", 2), ("Intel Quick Sync", 5)],
        3 => &[
            ("Automatic · best available", 0),
            ("NVIDIA NVENC", 3),
            ("Intel Quick Sync", 6),
            ("Software · Windows (CPU)", 7),
        ],
        _ => &[],
    };
    options
        .iter()
        .copied()
        .filter(|&(_, choice)| available.contains(&(choice as usize)))
        .collect()
}

/// Formats offered in Advanced: HEVC only when some encoder here can make it.
fn codecs_available(available: &[usize]) -> Vec<usize> {
    (0..CODEC_NAMES.len())
        .filter(|&codec| codec == 0 || !encoders_for(codec, available).is_empty())
        .collect()
}

fn string_model(values: impl Iterator<Item = &'static str>) -> slint::ModelRc<slint::SharedString> {
    std::rc::Rc::new(slint::VecModel::from(
        values.map(slint::SharedString::from).collect::<Vec<_>>(),
    ))
    .into()
}

pub(super) fn encoder_choices(ui: &MainWindow, state: &AppState) -> Vec<usize> {
    let explicit_gpu = if ui.get_gpu_choice() > 0 {
        state.gpus.get((ui.get_gpu_choice() - 1) as usize)
    } else {
        None
    };
    (0..ENCODER_NAMES.len())
        .filter(|&i| {
            if !(1..=6).contains(&i) {
                return true;
            }
            let supported = |gpu: &native::GpuInfo| {
                gpu.vendor == if i <= 3 { 0x10de } else { 0x8086 } && gpu.codecs[(i - 1) % 3]
            };
            explicit_gpu.map_or_else(|| state.gpus.iter().any(supported), supported)
        })
        .collect()
}
pub(super) fn update_encoding_labels(ui: &MainWindow, state: &AppState) {
    let choices = encoder_choices(ui, state);
    if !choices.contains(&(ui.get_encoder_choice() as usize)) {
        ui.set_encoder_choice(0);
    }
    let gpu =
        selected_gpu(ui, state).and_then(|index| state.gpus.iter().find(|gpu| gpu.index == index));
    let choice = ui.get_encoder_choice().clamp(0, 8) as usize;
    let (encoder, codec) = if choice == 0 {
        if let Some(gpu) = gpu {
            let c = if ui.get_prefer_h264() {
                gpu.codecs[2].then_some(2)
            } else {
                gpu.codecs.iter().position(|supported| *supported)
            };
            match (gpu.vendor, c) {
                (0x10de, Some(i)) => (
                    ENCODER_NAMES[1 + i],
                    [Codec::Av1, Codec::Hevc, Codec::H264][i],
                ),
                (0x8086, Some(i)) => (
                    ENCODER_NAMES[4 + i],
                    [Codec::Av1, Codec::Hevc, Codec::H264][i],
                ),
                _ => ("Media Foundation · H.264", Codec::H264),
            }
        } else {
            ("Media Foundation · H.264", Codec::H264)
        }
    } else {
        (ENCODER_NAMES[choice], preference(choice as i32).codec())
    };
    let available = if (1..=6).contains(&choice) {
        gpu.is_some_and(|gpu| {
            gpu.vendor == if choice <= 3 { 0x10de } else { 0x8086 } && gpu.codecs[(choice - 1) % 3]
        })
    } else {
        true
    };
    ui.set_encoder_available(available);
    ui.set_encoder_status(
        if !available {
            "Unavailable on the selected GPU / installed driver. Choose another encoder or GPU."
        } else if choice == 8 {
            "CPU encoding with rav1e. May skip frames if the CPU cannot keep up; a higher speed helps."
        } else if (4..=6).contains(&choice) {
            "Hardware encoding · installed Intel oneVPL / Media SDK runtime · NV12 readback."
        } else {
            ""
        }
        .into(),
    );
    let codec_index = codec_of(choice as i32, ui.get_prefer_h264());
    let codecs = codecs_available(&choices);
    ui.set_video_codec(codec_index as i32);
    ui.set_codec_names(string_model(codecs.iter().map(|&c| CODEC_NAMES[c])));
    ui.set_codec_list_index(codecs.iter().position(|&c| c == codec_index).unwrap_or(0) as i32);
    let encoders = encoders_for(codec_index, &choices);
    ui.set_encoder_names(string_model(encoders.iter().map(|&(name, _)| name)));
    ui.set_encoder_list_index(
        encoders
            .iter()
            .position(|&(_, c)| c == choice as i32)
            .unwrap_or(0) as i32,
    );
    ui.set_gpu_label(if choice >= 7 {
        "CPU".into()
    } else {
        gpu.map(|g| g.name.clone())
            .unwrap_or_else(|| "System default adapter".into())
            .into()
    });
    ui.set_nvenc_controls(encoder.starts_with("NVIDIA"));
    ui.set_keyframe_controls(
        encoder.starts_with("NVIDIA") || encoder.starts_with("Intel") || codec == Codec::Av1,
    );
    ui.set_encoder_tech(if encoder.starts_with("NVIDIA") { nvenc_summary(ui) }
        else if encoder.starts_with("Intel") { format!("Intel oneVPL / Media SDK · hardware · TU1 / up to 3 B-frames request · {} · {}s keyframes · NV12 readback", if ui.get_constant_bitrate() { "CBR" } else { "VBR" }, ui.get_keyframe_seconds()) }
        else if codec == Codec::Av1 { format!("rav1e 0.8.1 · Rust · speed {} · P-only · 8-frame lookahead · CPU NV12 input", ui.get_software_speed()) }
        else { "Windows Media Foundation · H.264 · driver / Windows rate control".into() }.into());
    ui.set_encoder_label(encoder.into());
    ui.set_codec_label(codec.name().into());
    let (width, height) = state
        .source
        .as_ref()
        .map(|s| fastrecorder_core::output_size(s.width, s.height, ui.get_output_height() as u32))
        .unwrap_or((1920, 1080));
    let recommended = recommended_bitrate_mbps(codec, width, height, ui.get_fps() as u32);
    let bitrate = match ui.get_bitrate_mode() {
        0 => (recommended as f64 * 1.5).round() as u32,
        2 => (recommended as f64 * 0.65).round() as u32,
        3 => ui.get_custom_bitrate() as u32,
        _ => recommended,
    }
    .clamp(1, 100);
    ui.set_bitrate_mbps(bitrate as i32);
    ui.set_bitrate_detail(
        if encoder.starts_with("NVIDIA") && ui.get_quality_mode() {
            format!(
                "CQP {} · {} · {width} × {height} · {} fps · variable bitrate / file size",
                ui.get_quality_level(),
                codec.name(),
                ui.get_fps()
            )
        } else {
            format!(
                "{bitrate} Mbps · {} · {width} × {height} · {} fps",
                codec.name(),
                ui.get_fps()
            )
        }
        .into(),
    );
    ui.set_fallback_detail(if choice == 0 { "Prefer AV1 → HEVC → H.264 on the selected GPU; then Media Foundation hardware H.264 → Windows software H.264. Startup failures are reported." }
        else { "Explicit selection; codec / vendor failures are reported without switching encoders." }.into());
}

fn set_encoder(ui: &MainWindow, state: &AppState, choice: i32, h264: bool) {
    ui.set_encoder_choice(choice);
    ui.set_prefer_h264(h264);
    update_encoding_labels(ui, state);
    persist_preferences(ui, state);
}

/// Wires encoder selection and probes GPU encoders off the UI thread.
pub(super) fn install(ui: &MainWindow, state: &Shared) -> std::io::Result<()> {
    let weak = ui.as_weak();
    let encoder_state = state.clone();
    ui.on_encoder_selected(move |index| {
        if let Some(ui) = weak.upgrade() {
            let state = encoder_state.locked();
            let codec = codec_of(ui.get_encoder_choice(), ui.get_prefer_h264());
            let encoders = encoders_for(codec, &encoder_choices(&ui, &state));
            if let Some(&(_, choice)) = usize::try_from(index)
                .ok()
                .and_then(|index| encoders.get(index))
            {
                set_encoder(&ui, &state, choice, codec == 3);
            }
        }
    });
    // Advanced → Format: pick the codec, then the best encoder here that makes it.
    let weak = ui.as_weak();
    let codec_state = state.clone();
    ui.on_codec_selected(move |index| {
        if let Some(ui) = weak.upgrade() {
            let state = codec_state.locked();
            let available = encoder_choices(&ui, &state);
            let Some(codec) = usize::try_from(index)
                .ok()
                .and_then(|index| codecs_available(&available).get(index).copied())
            else {
                return;
            };
            let choice = encoders_for(codec, &available)
                .first()
                .map_or(0, |&(_, choice)| choice);
            set_encoder(&ui, &state, choice, codec == 3);
        }
    });
    // Simple → Format: smaller files or plays everywhere; Pip picks the encoder.
    let weak = ui.as_weak();
    let format_state = state.clone();
    ui.on_format_selected(move |index| {
        if let Some(ui) = weak.upgrade() {
            set_encoder(&ui, &format_state.locked(), 0, index == 1);
        }
    });
    let weak = ui.as_weak();
    let hardware_state = state.clone();
    std::thread::Builder::new()
        .name("fastrecorder-capabilities".into())
        .spawn(move || {
            let inventory = native::gpu_inventory().map_err(|e| e.to_string());
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let mut state = hardware_state.locked();
                match inventory {
                    Ok(gpus) => {
                        let names = std::iter::once(slint::SharedString::from("Automatic"))
                            .chain(gpus.iter().map(|gpu| gpu.name.clone().into()))
                            .collect::<Vec<_>>();
                        ui.set_gpu_names(std::rc::Rc::new(slint::VecModel::from(names)).into());
                        ui.set_hardware_details(
                            gpus.iter()
                                .filter_map(|gpu| {
                                    let codecs = gpu
                                        .codecs
                                        .iter()
                                        .zip(["AV1", "HEVC", "H.264"])
                                        .filter_map(|(available, name)| available.then_some(name))
                                        .collect::<Vec<_>>();
                                    (!codecs.is_empty())
                                        .then(|| format!("{}\n{}", gpu.name, codecs.join(" · ")))
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n")
                                .into(),
                        );
                        state.gpus = gpus;
                        if let Some(name) = state.preferred_gpu.take() {
                            ui.set_gpu_choice(
                                state
                                    .gpus
                                    .iter()
                                    .position(|gpu| gpu.name == name)
                                    .map_or(0, |index| index as i32 + 1),
                            );
                        }
                        if let Some(choice) = state.preferred_encoder.take() {
                            ui.set_encoder_choice(choice);
                        }
                        update_encoding_labels(&ui, &state);
                    }
                    Err(error) => {
                        ui.set_hardware_details(error.clone().into());
                        notify_issue(&ui, "Couldn't check graphics hardware.", error);
                    }
                }
                ui.set_hardware_ready(true);
            });
        })?;
    Ok(())
}

/// The NVENC settings that will be requested, in OBS-like terms, for Info.
fn nvenc_summary(ui: &MainWindow) -> String {
    let low_latency = ui.get_low_latency();
    let rate = if ui.get_quality_mode() {
        format!("CQP {} · single pass", ui.get_quality_level())
    } else {
        let multipass = [
            "single pass",
            "two-pass quarter resolution",
            "two-pass full resolution",
        ][ui.get_multipass().clamp(0, 2) as usize];
        let peak = if ui.get_constant_bitrate() || ui.get_max_bitrate() == 0 {
            String::new()
        } else {
            format!(" · peak {} Mbps", ui.get_max_bitrate())
        };
        format!(
            "{}{peak} · {multipass}",
            if ui.get_constant_bitrate() {
                "CBR"
            } else {
                "VBR"
            }
        )
    };
    let b_frames = match (low_latency, ui.get_b_frames()) {
        (true, _) => "no B-frames".to_string(),
        (false, b) if b < 0 => "up to 2 B-frames".to_string(),
        (false, b) => format!("up to {b} B-frames"),
    };
    let aq = match (ui.get_spatial_aq(), ui.get_temporal_aq()) {
        (true, true) => "spatial + temporal AQ",
        (true, false) => "spatial AQ",
        (false, true) => "temporal AQ",
        (false, false) => "AQ off",
    };
    format!(
        "NVENC API 12.1 ABI · P{} · {} tuning · {aq} · {rate} · {b_frames} · {} · {}s keyframes · driver may reduce optional features",
        ui.get_nvenc_preset(),
        if low_latency {
            "low-latency"
        } else {
            "high-quality"
        },
        if ui.get_lookahead() && !low_latency {
            "8–16-frame lookahead"
        } else {
            "no lookahead"
        },
        ui.get_keyframe_seconds()
    )
}
