//! Video encoder selection and a uniform interface over every backend.
pub(crate) mod intel;
pub(crate) mod media_foundation;
pub(crate) mod nvenc;
pub(crate) mod software_av1;

use crate::gpu::sample_texture;
use fastrecorder_core::{EncoderPreference, RecordingConfig};
use media_foundation::create_writer;
use std::path::Path;
use windows::{
    Win32::{Graphics::Direct3D11::*, Media::MediaFoundation::*},
    core::Interface,
};

pub(crate) enum VideoEncoder {
    Nvenc(Box<crate::encode::nvenc::Nvenc>),
    Intel(Box<crate::encode::intel::Intel>),
    SoftwareAv1(Box<crate::encode::software_av1::SoftwareAv1>),
    Mf {
        writer: IMFSinkWriter,
        stream: u32,
        hardware: bool,
        label: String,
        fallback: Option<String>,
    },
}
impl VideoEncoder {
    #[allow(clippy::too_many_arguments)] // Native graphics handles and media setup share this boundary.
    pub(crate) fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        manager: &IMFDXGIDeviceManager,
        path: &Path,
        input: &IMFMediaType,
        width: u32,
        height: u32,
        config: &RecordingConfig,
    ) -> Result<Self, String> {
        let mut fallback = None;
        let automatic = config.encoder == EncoderPreference::Auto;
        let adapter: windows::Win32::Graphics::Dxgi::IDXGIDevice =
            device.cast().map_err(|e| e.to_string())?;
        let vendor = unsafe { adapter.GetAdapter().and_then(|a| a.GetDesc()) }
            .map_err(|e| e.to_string())?
            .VendorId;
        let candidates: Vec<fastrecorder_core::Codec> = if automatic {
            vec![
                fastrecorder_core::Codec::Av1,
                fastrecorder_core::Codec::Hevc,
                fastrecorder_core::Codec::H264,
            ]
        } else {
            vec![config.encoder.codec()]
        };
        if automatic
            || matches!(
                config.encoder,
                EncoderPreference::NvencAv1
                    | EncoderPreference::NvencHevc
                    | EncoderPreference::NvencH264
            )
        {
            if vendor == 0x10de {
                for codec in &candidates {
                    match crate::encode::nvenc::Nvenc::new(
                        device,
                        context,
                        path,
                        width,
                        height,
                        config.fps,
                        config.bitrate_for(*codec, width, height) * 1_000_000,
                        config.nvenc_preset,
                        config.keyframe_seconds,
                        config.constant_bitrate,
                        config.quality_qp,
                        *codec,
                    ) {
                        Ok(mut encoder) => {
                            encoder.fallback = match (fallback, encoder.fallback.take()) {
                                (Some(startup), Some(quality)) => {
                                    Some(format!("{startup}; {quality}"))
                                }
                                (startup, quality) => startup.or(quality),
                            };
                            return Ok(Self::Nvenc(Box::new(encoder)));
                        }
                        Err(error) if !automatic => return Err(error),
                        Err(error) => {
                            fallback = Some(format!(
                                "{}; NVENC {}: {error}",
                                fallback.unwrap_or_default(),
                                codec.name()
                            ))
                        }
                    }
                }
            } else if !automatic {
                return Err("NVENC requires an NVIDIA adapter".into());
            }
        }
        if automatic && vendor == 0x8086 {
            for codec in &candidates {
                match crate::encode::intel::Intel::new(device, path, width, height, config, *codec)
                {
                    Ok(mut encoder) => {
                        encoder.fallback = fallback;
                        return Ok(Self::Intel(Box::new(encoder)));
                    }
                    Err(error) => {
                        fallback = Some(format!(
                            "{}; Intel {}: {error}",
                            fallback.unwrap_or_default(),
                            codec.name()
                        ))
                    }
                }
            }
        }
        let bitrate = config.bitrate_for(fastrecorder_core::Codec::H264, width, height) * 1_000_000;
        if config.encoder == EncoderPreference::SoftwareAv1 {
            return crate::encode::software_av1::SoftwareAv1::new(path, width, height, config)
                .map(|encoder| Self::SoftwareAv1(Box::new(encoder)));
        }
        if matches!(
            config.encoder,
            EncoderPreference::IntelAv1
                | EncoderPreference::IntelHevc
                | EncoderPreference::IntelH264
        ) {
            return crate::encode::intel::Intel::new(
                device,
                path,
                width,
                height,
                config,
                config.encoder.codec(),
            )
            .map(|encoder| Self::Intel(Box::new(encoder)));
        }
        if config.encoder != EncoderPreference::SoftwareOnly {
            match create_writer(
                path,
                input,
                width,
                height,
                config.fps,
                Some(manager),
                bitrate,
            ) {
                Ok((writer, stream, label)) => {
                    return Ok(Self::Mf {
                        writer,
                        stream,
                        hardware: true,
                        label: format!("Media Foundation · {label} · H.264 hardware"),
                        fallback,
                    });
                }
                Err(error) if config.encoder == EncoderPreference::HardwareH264 => {
                    return Err(format!("Hardware H.264 unavailable: {error}"));
                }
                Err(error) => {
                    fallback = Some(format!(
                        "{}; hardware H.264 unavailable: {error}",
                        fallback.unwrap_or_default()
                    ));
                }
            }
        }
        let (writer, stream, label) =
            create_writer(path, input, width, height, config.fps, None, bitrate)
                .map_err(|e| format!("Windows software encoding could not start: {e}"))?;
        Ok(Self::Mf {
            writer,
            stream,
            hardware: false,
            label: format!("Media Foundation · {label} · H.264 software"),
            fallback,
        })
    }
    pub(crate) fn hardware(&self) -> bool {
        match self {
            Self::Nvenc(_) | Self::Intel(_) => true,
            Self::SoftwareAv1(_) => false,
            Self::Mf { hardware, .. } => *hardware,
        }
    }
    pub(crate) fn cpu_input(&self) -> bool {
        matches!(
            self,
            Self::Intel(_)
                | Self::SoftwareAv1(_)
                | Self::Mf {
                    hardware: false,
                    ..
                }
        )
    }
    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Nvenc(encoder) => &encoder.label,
            Self::Intel(encoder) => &encoder.label,
            Self::SoftwareAv1(_) => "rav1e · AV1 software · Rust · speed 8 · 8-frame lookahead",
            Self::Mf { label, .. } => label,
        }
    }
    pub(crate) fn codec(&self) -> &str {
        match self {
            Self::Nvenc(encoder) => encoder.codec.name(),
            Self::Intel(encoder) => encoder.codec.name(),
            Self::SoftwareAv1(_) => "AV1",
            Self::Mf { .. } => "H.264",
        }
    }
    pub(crate) fn fallback(&self) -> Option<&str> {
        match self {
            Self::Nvenc(encoder) => encoder.fallback.as_deref(),
            Self::Intel(encoder) => encoder.fallback.as_deref(),
            Self::SoftwareAv1(_) => None,
            Self::Mf { fallback, .. } => fallback.as_deref(),
        }
    }
    pub(crate) fn write(&mut self, sample: &IMFSample, index: u64, _: u32) -> Result<(), String> {
        match self {
            Self::Nvenc(encoder) => {
                let (texture, subresource) = sample_texture(sample).map_err(|e| e.to_string())?;
                encoder.write(&texture, subresource, index)
            }
            Self::Intel(encoder) => encoder.write(sample, index),
            Self::SoftwareAv1(encoder) => encoder.write(sample, index),
            Self::Mf { writer, stream, .. } => unsafe {
                writer
                    .WriteSample(*stream, sample)
                    .map_err(|e| format!("Encoding failed: {e}"))
            },
        }
    }
    pub(crate) fn finalize(
        self,
        path: &Path,
        audio: Option<&fastrecorder_mp4::AacTrack>,
    ) -> Result<Option<String>, String> {
        match self {
            Self::Nvenc(mut encoder) => encoder.finalize(audio),
            Self::Intel(mut encoder) => encoder.finalize(audio).map(|()| None),
            Self::SoftwareAv1(mut encoder) => encoder.finalize(audio).map(|()| None),
            Self::Mf { writer, .. } => {
                unsafe {
                    writer.Finalize().map_err(|e| e.to_string())?;
                }
                drop(writer); // Release the native file handle before adding AAC.
                if let Some(audio) = audio {
                    fastrecorder_mp4::attach_audio(path, audio).map_err(|e| e.to_string())?;
                }
                Ok(None)
            }
        }
    }
}
