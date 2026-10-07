//! One bounded NVENC input/output slot: registered texture, bitstream and event.
use super::{api::bindings::*, session::Session};
use std::{ptr, rc::Rc};
use windows::{
    Win32::Graphics::{Direct3D11::*, Dxgi::Common::*},
    core::Interface,
};

pub(super) struct Slot {
    pub(super) session: Rc<Session>,
    pub(super) input: ID3D11Texture2D,
    pub(super) registered: NV_ENC_REGISTERED_PTR,
    pub(super) bitstream: NV_ENC_OUTPUT_PTR,
    pub(super) mapped: NV_ENC_INPUT_PTR,
    pub(super) timestamp: u64,
}
impl Slot {
    pub(super) fn new(
        session: Rc<Session>,
        device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let mut slot = Self {
            input: crate::gpu::texture(
                device,
                width,
                height,
                DXGI_FORMAT_NV12,
                D3D11_USAGE_DEFAULT,
                (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
                0,
            )
            .map_err(|e| e.to_string())?,
            session,
            registered: ptr::null_mut(),
            bitstream: ptr::null_mut(),
            mapped: ptr::null_mut(),
            timestamp: 0,
        };
        unsafe {
            let s = &slot.session;
            let mut resource = NV_ENC_REGISTER_RESOURCE::default();
            resource.version = NV_ENC_REGISTER_RESOURCE_VER;
            resource.resourceType = NV_ENC_INPUT_RESOURCE_TYPE::NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
            resource.resourceToRegister = slot.input.as_raw();
            resource.width = width;
            resource.height = height;
            resource.bufferFormat = NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_NV12;
            resource.bufferUsage = NV_ENC_BUFFER_USAGE::NV_ENC_INPUT_IMAGE;
            s.check((s
                .api
                .nvEncRegisterResource
                .ok_or("Missing GPU resource API")?)(
                s.handle, &mut resource
            ))?;
            slot.registered = resource.registeredResource;
            let mut output = NV_ENC_CREATE_BITSTREAM_BUFFER::default();
            output.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
            s.check((s
                .api
                .nvEncCreateBitstreamBuffer
                .ok_or("Missing output buffer API")?)(
                s.handle, &mut output
            ))?;
            slot.bitstream = output.bitstreamBuffer;
        }
        Ok(slot)
    }
    pub(super) fn unmap(&mut self) -> Result<(), String> {
        if self.mapped.is_null() {
            return Ok(());
        }
        unsafe {
            self.session.check((self
                .session
                .api
                .nvEncUnmapInputResource
                .ok_or("Missing unmap API")?)(
                self.session.handle, self.mapped
            ))?;
        }
        self.mapped = ptr::null_mut();
        Ok(())
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        let _ = self.unmap();
        unsafe {
            if !self.bitstream.is_null()
                && let Some(destroy) = self.session.api.nvEncDestroyBitstreamBuffer
            {
                let _ = destroy(self.session.handle, self.bitstream);
            }
            if !self.registered.is_null()
                && let Some(unregister) = self.session.api.nvEncUnregisterResource
            {
                let _ = unregister(self.session.handle, self.registered);
            }
        }
    }
}
