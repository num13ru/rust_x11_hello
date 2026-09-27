//! Display-backend-independent presentation boundary and state.

mod cache;

use anyhow::{Result, ensure};
use paper_protocol::{PixelFormat, validate_gray8_pixels, validate_mono1_pixels};

pub(crate) use cache::RemoteFrameCache;

/// Validated, borrowed remote frame plus its presentation diagnostics.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RemoteFrame<'a> {
    frame_id: u64,
    width: u16,
    height: u16,
    pixel_format: PixelFormat,
    stride: usize,
    pixels: &'a [u8],
    receive_decode_us: u128,
}

impl<'a> RemoteFrame<'a> {
    pub(crate) fn new(
        frame_id: u64,
        width: u16,
        height: u16,
        pixel_format: PixelFormat,
        stride: usize,
        pixels: &'a [u8],
        receive_decode_us: u128,
    ) -> Result<Self> {
        let validated_stride = match pixel_format {
            PixelFormat::Mono1 => validate_mono1_pixels(width, height, pixels)?,
            PixelFormat::Gray8 => validate_gray8_pixels(width, height, pixels)?,
        };
        ensure!(
            stride == validated_stride,
            "{pixel_format:?} frame stride mismatch: expected {validated_stride}, got {stride}"
        );
        Ok(Self {
            frame_id,
            width,
            height,
            pixel_format,
            stride,
            pixels,
            receive_decode_us,
        })
    }

    pub(crate) fn frame_id(self) -> u64 {
        self.frame_id
    }

    pub(crate) fn width(self) -> u16 {
        self.width
    }

    pub(crate) fn height(self) -> u16 {
        self.height
    }

    pub(crate) fn pixel_format(self) -> PixelFormat {
        self.pixel_format
    }

    pub(crate) fn stride(self) -> usize {
        self.stride
    }

    pub(crate) fn pixels(self) -> &'a [u8] {
        self.pixels
    }

    pub(crate) fn receive_decode_us(self) -> u128 {
        self.receive_decode_us
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CachedFrameMetadata {
    frame_id: u64,
    dimensions: (u16, u16),
    pixel_format: PixelFormat,
    stride: usize,
}

impl CachedFrameMetadata {
    pub(crate) fn new(
        frame_id: u64,
        dimensions: (u16, u16),
        pixel_format: PixelFormat,
        stride: usize,
    ) -> Self {
        Self {
            frame_id,
            dimensions,
            pixel_format,
            stride,
        }
    }

    pub(crate) fn frame_id(self) -> u64 {
        self.frame_id
    }

    pub(crate) fn dimensions(self) -> (u16, u16) {
        self.dimensions
    }

    pub(crate) fn pixel_format(self) -> PixelFormat {
        self.pixel_format
    }

    pub(crate) fn stride(self) -> usize {
        self.stride
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RedrawCause {
    SurfaceDamage,
    GeometryChange,
}

/// Operations the PaperPad runtime needs from any display implementation.
///
/// Remote-frame conversion and presentation failures remain non-fatal and are
/// diagnosed by the backend. Local system-UI failures are returned because the
/// device-owned Exit control must remain renderable independently of PaperSpoon.
pub(crate) trait DisplayBackend {
    fn dimensions(&self) -> (u16, u16);

    fn set_dimensions(&mut self, dimensions: (u16, u16)) -> Result<Option<CachedFrameMetadata>>;

    fn display_remote_frame(&mut self, frame: RemoteFrame<'_>);

    fn redraw_cached_frame(&mut self, cause: RedrawCause);

    fn draw_system_ui(&mut self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_frame_validates_format_and_stride_before_crossing_backend_boundary() {
        let frame = RemoteFrame::new(7, 9, 1, PixelFormat::Mono1, 2, &[0xaa, 0x80], 13)
            .expect("valid frame");
        assert_eq!(frame.frame_id(), 7);
        assert_eq!((frame.width(), frame.height()), (9, 1));
        assert_eq!(frame.pixel_format(), PixelFormat::Mono1);
        assert_eq!(frame.stride(), 2);
        assert_eq!(frame.pixels(), &[0xaa, 0x80]);
        assert_eq!(frame.receive_decode_us(), 13);

        assert!(RemoteFrame::new(8, 9, 1, PixelFormat::Mono1, 2, &[0xaa, 0x81], 0).is_err());
        assert!(RemoteFrame::new(9, 9, 1, PixelFormat::Mono1, 1, &[0xaa, 0x80], 0).is_err());

        let gray8 = RemoteFrame::new(10, 2, 2, PixelFormat::Gray8, 2, &[0, 64, 128, 255], 21)
            .expect("valid Gray8 frame");
        assert_eq!(gray8.pixel_format(), PixelFormat::Gray8);
        assert_eq!(gray8.stride(), 2);
        assert_eq!(gray8.pixels(), &[0, 64, 128, 255]);
        assert!(RemoteFrame::new(11, 2, 2, PixelFormat::Gray8, 1, &[0, 64, 128, 255], 0,).is_err());
        assert!(RemoteFrame::new(12, 2, 2, PixelFormat::Gray8, 2, &[0, 64, 128], 0).is_err());
    }
}
