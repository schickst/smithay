//! Module for Buffers created using [libgbm](reexports::gbm)

use super::{
    dmabuf::{AsDmabuf, Dmabuf, DmabufFlags, MAX_PLANES},
    Allocator, Buffer, Format, Fourcc, Modifier,
};
use gbm::{BufferObject as GbmBuffer, BufferObjectFlags, Device as GbmDevice};
use std::os::unix::io::AsRawFd;

impl<A: AsRawFd + 'static, T> Allocator<GbmBuffer<T>> for GbmDevice<A> {
    type Error = std::io::Error;

    fn create_buffer(&mut self, width: u32, height: u32, format: Format) -> std::io::Result<GbmBuffer<T>> {
        if format.modifier == Modifier::Invalid || format.modifier == Modifier::Linear {
            let mut usage = BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING;
            if format.modifier == Modifier::Linear {
                usage |= BufferObjectFlags::LINEAR;
            }
            self.create_buffer_object(width, height, format.code, usage)
        } else {
            self.create_buffer_object_with_modifiers(
                width,
                height,
                format.code,
                Some(format.modifier).into_iter(),
            )
        }
    }
}

impl<T> Buffer for GbmBuffer<T> {
    fn width(&self) -> u32 {
        self.width().unwrap_or(0)
    }

    fn height(&self) -> u32 {
        self.height().unwrap_or(0)
    }

    fn format(&self) -> Format {
        Format {
            code: self.format().unwrap_or(Fourcc::Argb8888), // we got to return something, but this should never happen anyway
            modifier: self.modifier().unwrap_or(Modifier::Invalid),
        }
    }
}

/// Errors during conversion to a dmabuf handle from a gbm buffer object
#[derive(thiserror::Error, Debug)]
pub enum GbmConvertError {
    /// The gbm device was destroyed
    #[error("The gbm device was destroyed")]
    DeviceDestroyed(#[from] gbm::DeviceDestroyedError),
    /// The buffer consists out of multiple file descriptions, which is currently unsupported
    #[error("Buffer consists out of multiple file descriptors, which is currently unsupported")]
    UnsupportedBuffer,
    /// The conversion returned an invalid file descriptor
    #[error("Buffer returned invalid file descriptor")]
    InvalidFD,
}

impl<T> AsDmabuf for GbmBuffer<T> {
    type Error = GbmConvertError;

    fn export(&self) -> Result<Dmabuf, GbmConvertError> {
        let planes = self.plane_count()? as i32;

        //TODO switch to gbm_bo_get_plane_fd when it lands
        let mut iter = (0i32..planes).map(|i| self.handle_for_plane(i));
        let first = iter.next().expect("Encountered a buffer with zero planes");
        // check that all handles are the same
        let handle = iter.try_fold(first, |first, next| {
            if let (Ok(next), Ok(first)) = (next, first) {
                if unsafe { next.u64_ == first.u64_ } {
                    return Some(Ok(first));
                }
            }
            None
        });
        if handle.is_none() {
            // GBM is lacking a function to get a FD for a given plane. Instead,
            // check all planes have the same handle. We can't use
            // drmPrimeHandleToFD because that messes up handle ref'counting in
            // the user-space driver.
            return Err(GbmConvertError::UnsupportedBuffer); //TODO
        }

        if self.fd()? == 0 {
            return Err(GbmConvertError::InvalidFD);
        }

        let mut builder = Dmabuf::new_from_buffer(self, DmabufFlags::empty());
        for idx in 0..planes {
            builder.add_plane(
                self.fd()?,
                idx as u32,
                self.offset(idx)?,
                self.stride_for_plane(idx)?,
                self.modifier()?,
            );
        }
        Ok(builder.build().unwrap())
    }
}

impl Dmabuf {
    /// Import a Dmabuf using libgbm, creating a gbm Buffer Object to the same underlying data.
    pub fn import<A: AsRawFd + 'static, T>(
        &self,
        gbm: &GbmDevice<A>,
        usage: BufferObjectFlags,
    ) -> std::io::Result<GbmBuffer<T>> {
        let mut handles = [0; MAX_PLANES];
        for (i, handle) in self.handles().take(MAX_PLANES).enumerate() {
            handles[i] = handle;
        }
        let mut strides = [0i32; MAX_PLANES];
        for (i, stride) in self.strides().take(MAX_PLANES).enumerate() {
            strides[i] = stride as i32;
        }
        let mut offsets = [0i32; MAX_PLANES];
        for (i, offset) in self.offsets().take(MAX_PLANES).enumerate() {
            offsets[i] = offset as i32;
        }

        if self.has_modifier() || self.num_planes() > 1 || self.offsets().next().unwrap() != 0 {
            gbm.import_buffer_object_from_dma_buf_with_modifiers(
                self.num_planes() as u32,
                handles,
                self.width(),
                self.height(),
                self.format().code,
                usage,
                strides,
                offsets,
                self.format().modifier,
            )
        } else {
            gbm.import_buffer_object_from_dma_buf(
                handles[0],
                self.width(),
                self.height(),
                strides[0] as u32,
                self.format().code,
                if self.format().modifier == Modifier::Linear {
                    usage | BufferObjectFlags::LINEAR
                } else {
                    usage
                },
            )
        }
    }
}
