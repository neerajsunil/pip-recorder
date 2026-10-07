//! COM/Media Foundation lifetime guards and endpoint enumeration.
use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::{Audio::*, MediaFoundation::*},
        System::{
            Com::{StructuredStorage::*, *},
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
    },
    core::Result as WinResult,
};

#[derive(Clone)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}
pub struct AudioInventory {
    pub desktop: Vec<AudioDevice>,
    pub microphones: Vec<AudioDevice>,
}

pub(super) struct Apartment;
impl Apartment {
    pub(super) fn new() -> WinResult<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
pub(super) struct MediaFoundation;
impl MediaFoundation {
    pub(super) fn new() -> WinResult<Self> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        }
        Ok(Self)
    }
}
impl Drop for MediaFoundation {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}
pub(super) fn device_name(device: &IMMDevice) -> WinResult<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ)?;
        let mut value = store.GetValue(&PKEY_Device_FriendlyName)?;
        let text = PropVariantToStringAlloc(&value);
        let _ = PropVariantClear(&mut value);
        let text = text?;
        let result = text.to_string();
        CoTaskMemFree(Some(text.0.cast()));
        Ok(result?)
    }
}
pub fn audio_inventory() -> Result<AudioInventory, String> {
    let _apartment = Apartment::new().map_err(|e| e.to_string())?;
    let enumerate = || -> WinResult<_> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)?;
            let list = |flow| -> WinResult<Vec<AudioDevice>> {
                let devices = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
                let mut result = Vec::new();
                for index in 0..devices.GetCount()? {
                    let device = devices.Item(index)?;
                    let raw = device.GetId()?;
                    let id = raw.to_string();
                    CoTaskMemFree(Some(raw.0.cast()));
                    result.push(AudioDevice {
                        id: id?,
                        name: device_name(&device)?,
                    });
                }
                result.sort_by_key(|device| device.name.to_lowercase());
                Ok(result)
            };
            Ok(AudioInventory {
                desktop: list(eRender)?,
                microphones: list(eCapture)?,
            })
        }
    };
    enumerate().map_err(|e| format!("Could not list audio devices: {e}"))
}
