//! Output codecs and the encoder a user can request.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncoderPreference {
    #[default]
    Auto,
    SoftwareOnly,
    NvencAv1,
    HardwareH264,
    NvencHevc,
    NvencH264,
    IntelAv1,
    IntelHevc,
    IntelH264,
    SoftwareAv1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Av1,
    Hevc,
    H264,
}
impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Self::Av1 => "AV1",
            Self::Hevc => "HEVC",
            Self::H264 => "H.264",
        }
    }
}
impl EncoderPreference {
    pub fn codec(self) -> Codec {
        match self {
            Self::NvencAv1 | Self::IntelAv1 | Self::SoftwareAv1 => Codec::Av1,
            Self::NvencHevc | Self::IntelHevc => Codec::Hevc,
            _ => Codec::H264,
        }
    }
}
