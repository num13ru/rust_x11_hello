//! Experimental MXCFB sibling of the X11 display backend.

use std::fs::File;

use anyhow::{Context, Result, ensure};

use crate::display::{
    CachedFrameMetadata, DisplayBackend, RedrawCause, RemoteFrame, RemoteFrameCache,
};

use super::controller;
use super::opened::OpenedFramebuffer;
use super::present::{present_remote_with, present_system_ui_with};
use super::update_abi::UpdateDataMtk;
use super::update_marker::UpdateMarkerSequence;

type SubmitUpdate = fn(&File, &mut UpdateDataMtk, u32, u32) -> Result<()>;

pub(crate) struct MxcfbDisplayBackend {
    opened: OpenedFramebuffer,
    markers: UpdateMarkerSequence,
    remote_frame_cache: RemoteFrameCache<Vec<u8>>,
    submit_update: SubmitUpdate,
}

impl MxcfbDisplayBackend {
    pub(crate) fn open() -> Result<Self> {
        let opened = OpenedFramebuffer::open().context("initialize MXCFB display backend")?;
        Ok(Self::from_opened(opened, controller::submit_update))
    }

    fn from_opened(opened: OpenedFramebuffer, submit_update: SubmitUpdate) -> Self {
        let spec = opened.info.spec;
        eprintln!(
            "display backend=mxcfb width={} height={} bpp={} line_length={} visual={} xoffset={} yoffset={}",
            spec.visible_width,
            spec.visible_height,
            spec.bits_per_pixel,
            spec.line_length,
            spec.visual,
            spec.xoffset,
            spec.yoffset
        );
        Self {
            opened,
            markers: UpdateMarkerSequence::for_process(),
            remote_frame_cache: RemoteFrameCache::default(),
            submit_update,
        }
    }
}

impl DisplayBackend for MxcfbDisplayBackend {
    fn dimensions(&self) -> (u16, u16) {
        self.opened.info.screen.physical_size()
    }

    fn set_dimensions(&mut self, dimensions: (u16, u16)) -> Result<Option<CachedFrameMetadata>> {
        ensure!(
            dimensions == self.dimensions(),
            "MXCFB X11 window geometry {:?} differs from framebuffer {:?}",
            dimensions,
            self.dimensions()
        );
        Ok(None)
    }

    fn display_remote_frame(&mut self, frame: RemoteFrame<'_>) {
        let viewport = self.opened.info.screen.remote_viewport;
        if (frame.width(), frame.height()) != (viewport.width, viewport.height) {
            eprintln!(
                "mxcfb frame rejected id={} width={} height={} expected={}x{}",
                frame.frame_id(),
                frame.width(),
                frame.height(),
                viewport.width,
                viewport.height
            );
            return;
        }

        // Reserve the cache payload before presentation. After a successful
        // kernel submission, cache replacement itself cannot fail to allocate.
        let mut cached_pixels = Vec::new();
        if let Err(error) = cached_pixels.try_reserve_exact(frame.pixels().len()) {
            eprintln!(
                "mxcfb frame cache allocation failed id={}: {error}",
                frame.frame_id()
            );
            return;
        }
        cached_pixels.extend_from_slice(frame.pixels());

        match present_remote_with(
            &mut self.opened,
            &mut self.markers,
            frame,
            self.submit_update,
        ) {
            Ok(submitted) => {
                self.remote_frame_cache.replace(
                    frame.frame_id(),
                    frame.width(),
                    frame.height(),
                    cached_pixels,
                );
                eprintln!(
                    "mxcfb frame submitted id={} marker={} region=({},{} {}x{}) cache=updated receive_decode_us={}",
                    frame.frame_id(),
                    submitted.marker,
                    submitted.region.x,
                    submitted.region.y,
                    submitted.region.width,
                    submitted.region.height,
                    frame.receive_decode_us()
                );
            }
            Err(error) => eprintln!("mxcfb frame failed id={}: {error:#}", frame.frame_id()),
        }
    }

    fn redraw_cached_frame(&mut self, cause: RedrawCause) {
        let Some(cached) = self.remote_frame_cache.current() else {
            return;
        };
        let (width, height) = cached.dimensions();
        let frame = match RemoteFrame::new(
            cached.frame_id(),
            width,
            height,
            usize::from(width).div_ceil(8),
            cached.payload(),
            0,
        ) {
            Ok(frame) => frame,
            Err(error) => {
                eprintln!(
                    "mxcfb cached frame invalid id={}: {error:#}",
                    cached.frame_id()
                );
                return;
            }
        };
        match present_remote_with(
            &mut self.opened,
            &mut self.markers,
            frame,
            self.submit_update,
        ) {
            Ok(submitted) => eprintln!(
                "mxcfb frame restored id={} cause={cause:?} marker={} region=({},{} {}x{})",
                frame.frame_id(),
                submitted.marker,
                submitted.region.x,
                submitted.region.y,
                submitted.region.width,
                submitted.region.height
            ),
            Err(error) => eprintln!(
                "mxcfb frame restore failed id={} cause={cause:?}: {error:#}",
                frame.frame_id()
            ),
        }
    }

    fn draw_system_ui(&mut self) -> Result<()> {
        let submitted =
            present_system_ui_with(&mut self.opened, &mut self.markers, self.submit_update)?;
        eprintln!(
            "mxcfb local Exit submitted marker={} region=({},{} {}x{})",
            submitted.marker,
            submitted.region.x,
            submitted.region.y,
            submitted.region.width,
            submitted.region.height
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;

    use super::*;
    use crate::mxcfb::layout::FramebufferGeometry;
    use crate::mxcfb::mapped::WritableFramebuffer;
    use crate::mxcfb::metadata::FramebufferInfo;
    use crate::mxcfb::pixels::FramebufferSpec;
    use crate::ui::screen::ScreenLayout;

    fn fake_backend(submit_update: SubmitUpdate) -> MxcfbDisplayBackend {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/zero")
            .unwrap();
        let geometry = FramebufferGeometry {
            xres: 64,
            yres: 73,
            xres_virtual: 64,
            yres_virtual: 73,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 8,
            line_length: 68,
            smem_len: 68 * 73,
        };
        // SAFETY: /dev/zero supports this validated 4,964-byte mapping for
        // the lifetime of the descriptor retained by OpenedFramebuffer.
        let mapping = unsafe { WritableFramebuffer::map_visible(&file, geometry) }.unwrap();
        let info = FramebufferInfo {
            geometry,
            spec: FramebufferSpec {
                visible_width: 64,
                visible_height: 73,
                virtual_width: 64,
                virtual_height: 73,
                xoffset: 0,
                yoffset: 0,
                line_length: 68,
                memory_len: 68 * 73,
                kind: 0,
                visual: 1,
                bits_per_pixel: 8,
                grayscale: 1,
                nonstd: 0,
            },
            screen: ScreenLayout::new(64, 73).unwrap(),
            mapping_len: 68 * 73,
        };
        MxcfbDisplayBackend::from_opened(
            OpenedFramebuffer {
                mapping,
                file,
                info,
            },
            submit_update,
        )
    }

    fn accept_update(_: &File, _: &mut UpdateDataMtk, _: u32, _: u32) -> Result<()> {
        Ok(())
    }

    fn reject_update(_: &File, _: &mut UpdateDataMtk, _: u32, _: u32) -> Result<()> {
        anyhow::bail!("simulated update rejection")
    }

    #[test]
    fn cache_changes_only_after_accepted_remote_update() {
        let mut backend = fake_backend(accept_update);
        let first_pixels = [0x80; 8];
        let first = RemoteFrame::new(1, 64, 1, 8, &first_pixels, 0).unwrap();
        backend.display_remote_frame(first);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 1);

        let wrong_pixels = [0x80, 0x00];
        let wrong = RemoteFrame::new(2, 9, 1, 2, &wrong_pixels, 0).unwrap();
        backend.display_remote_frame(wrong);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 1);

        backend.submit_update = reject_update;
        let second_pixels = [0x00; 8];
        let second = RemoteFrame::new(3, 64, 1, 8, &second_pixels, 0).unwrap();
        backend.display_remote_frame(second);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 1);
        assert_eq!(
            backend.remote_frame_cache.current().unwrap().payload(),
            &first_pixels
        );
        backend.redraw_cached_frame(RedrawCause::SurfaceDamage);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 1);
    }

    #[test]
    fn fixed_geometry_and_local_exit_failures_are_explicit() {
        let mut backend = fake_backend(accept_update);
        assert_eq!(backend.dimensions(), (64, 73));
        assert_eq!(backend.set_dimensions((64, 73)).unwrap(), None);
        assert!(backend.set_dimensions((65, 73)).is_err());
        backend.draw_system_ui().unwrap();
        backend.submit_update = reject_update;
        assert!(backend.draw_system_ui().is_err());
    }
}
