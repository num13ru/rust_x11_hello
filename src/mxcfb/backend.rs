//! Experimental MXCFB sibling of the X11 display backend.

use std::fs::File;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};

use crate::display::{
    CachedFrameMetadata, DisplayBackend, RedrawCause, RemoteFrame, RemoteFrameCache,
};

use super::controller;
use super::dirty::changed_region;
use super::opened::OpenedFramebuffer;
use super::present::{present_remote_region_with, present_remote_with, present_system_ui_with};
use super::update_abi::UpdateDataMtk;
use super::update_marker::UpdateMarkerSequence;
use super::update_request::{MxcfbWaveform, next_full_screen_gc16_request, next_remote_request};

type SubmitUpdate = fn(&File, &mut UpdateDataMtk, u32, u32) -> Result<()>;
type WaitUpdate = fn(&File, u32) -> Result<()>;
const WHITE: u8 = 0xff;
const STARTUP_REASSERT_DELAY: Duration = Duration::from_millis(750);

pub(crate) struct MxcfbDisplayBackend {
    opened: OpenedFramebuffer,
    markers: UpdateMarkerSequence,
    remote_frame_cache: RemoteFrameCache<Vec<u8>>,
    submit_update: SubmitUpdate,
    wait_update: WaitUpdate,
    initial_screen: Option<Vec<u8>>,
    screen_modified: bool,
    startup_refresh_at: Option<Instant>,
    last_connected: Option<bool>,
}

impl MxcfbDisplayBackend {
    pub(crate) fn open() -> Result<Self> {
        let opened = OpenedFramebuffer::open().context("initialize MXCFB display backend")?;
        let mut backend = Self::from_opened(
            opened,
            controller::submit_update,
            controller::wait_for_update_complete,
        )?;
        backend
            .clear_remote_viewport()
            .context("clear MXCFB remote viewport during startup")?;
        backend.startup_refresh_at = Some(Instant::now() + STARTUP_REASSERT_DELAY);
        Ok(backend)
    }

    fn from_opened(
        opened: OpenedFramebuffer,
        submit_update: SubmitUpdate,
        wait_update: WaitUpdate,
    ) -> Result<Self> {
        let spec = opened.info.spec;
        let initial_screen = opened
            .mapping
            .snapshot_visible()
            .context("snapshot framebuffer before MXCFB PaperPad")?;
        eprintln!(
            "display backend=mxcfb width={} height={} bpp={} line_length={} visual={} xoffset={} yoffset={} snapshot_bytes={}",
            spec.visible_width,
            spec.visible_height,
            spec.bits_per_pixel,
            spec.line_length,
            spec.visual,
            spec.xoffset,
            spec.yoffset,
            initial_screen.len()
        );
        Ok(Self {
            opened,
            markers: UpdateMarkerSequence::for_process(),
            remote_frame_cache: RemoteFrameCache::default(),
            submit_update,
            wait_update,
            initial_screen: Some(initial_screen),
            screen_modified: false,
            startup_refresh_at: None,
            last_connected: None,
        })
    }

    fn restore_initial_screen(&mut self) -> Result<()> {
        if !self.screen_modified {
            return Ok(());
        }
        let snapshot = self
            .initial_screen
            .as_deref()
            .context("MXCFB PaperPad initial framebuffer snapshot is missing")?;
        self.opened
            .mapping
            .restore_visible(snapshot)
            .context("restore pre-PaperPad framebuffer bytes")?;

        let spec = self.opened.info.spec;
        let mut request = next_full_screen_gc16_request(
            spec.visible_width,
            spec.visible_height,
            &mut self.markers,
        )
        .context("prepare full-screen MXCFB PaperPad restoration refresh")?;
        let marker = request.update_marker;
        (self.submit_update)(
            &self.opened.file,
            &mut request,
            spec.visible_width,
            spec.visible_height,
        )
        .context("submit full-screen MXCFB PaperPad restoration refresh")?;
        let wait_started = Instant::now();
        (self.wait_update)(&self.opened.file, marker)
            .context("wait for full-screen MXCFB PaperPad restoration refresh")?;
        eprintln!(
            "mxcfb cleanup restored_snapshot_bytes={} marker={} wait_us={} ioctl_completion_proven=false visual_completion_verified=false",
            snapshot.len(),
            marker,
            wait_started.elapsed().as_micros()
        );
        self.screen_modified = false;
        self.initial_screen = None;
        Ok(())
    }

    fn clear_remote_viewport(&mut self) -> Result<()> {
        let info = &self.opened.info;
        let mut request = next_remote_request(
            info.screen,
            info.spec.visible_width,
            info.spec.visible_height,
            &mut self.markers,
            MxcfbWaveform::Gc16,
        )
        .context("prepare full MXCFB startup clear update")?;
        self.screen_modified = true;
        self.opened
            .mapping
            .fill_solid_region(info.screen.remote_viewport, WHITE, info.spec)
            .context("fill MXCFB remote viewport white during startup")?;
        let marker = request.update_marker;
        (self.submit_update)(
            &self.opened.file,
            &mut request,
            info.spec.visible_width,
            info.spec.visible_height,
        )
        .context("submit full MXCFB startup clear update")?;
        eprintln!(
            "mxcfb startup remote viewport cleared value=0xff marker={} region=({},{} {}x{})",
            marker,
            info.screen.remote_viewport.x,
            info.screen.remote_viewport.y,
            info.screen.remote_viewport.width,
            info.screen.remote_viewport.height
        );
        Ok(())
    }

    fn present_cached_remote(&mut self, reason: &str) -> Result<bool> {
        let Some(cached) = self.remote_frame_cache.current() else {
            return Ok(false);
        };
        let (width, height) = cached.dimensions();
        let frame = RemoteFrame::new(
            cached.frame_id(),
            width,
            height,
            cached.pixel_format(),
            cached.stride(),
            cached.payload(),
            0,
        )
        .context("reconstruct cached MXCFB remote frame")?;

        self.screen_modified = true;
        let submitted = present_remote_with(
            &mut self.opened,
            &mut self.markers,
            frame,
            self.submit_update,
        )?;
        eprintln!(
            "mxcfb cached frame submitted id={} format={:?} marker={} region=({},{} {}x{}) reason={reason}",
            frame.frame_id(),
            frame.pixel_format(),
            submitted.marker,
            submitted.region.x,
            submitted.region.y,
            submitted.region.width,
            submitted.region.height
        );
        Ok(true)
    }
}

impl DisplayBackend for MxcfbDisplayBackend {
    fn dimensions(&self) -> (u16, u16) {
        self.opened.info.screen.physical_size()
    }

    fn pixel_formats(&self) -> paper_protocol::V2PixelFormats {
        paper_protocol::V2PixelFormats::MONO1.with(paper_protocol::PixelFormat::Gray8)
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

        let previous = self
            .remote_frame_cache
            .current()
            .filter(|cached| {
                cached.dimensions() == (frame.width(), frame.height())
                    && cached.pixel_format() == frame.pixel_format()
                    && cached.stride() == frame.stride()
            })
            .map(|cached| cached.payload().as_slice());
        let dirty = changed_region(
            frame.width(),
            frame.height(),
            frame.pixel_format(),
            frame.stride(),
            previous,
            frame.pixels(),
        );

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

        let Some(dirty) = dirty else {
            self.remote_frame_cache.replace(frame, cached_pixels);
            eprintln!(
                "mxcfb frame unchanged id={} format={:?} cache=updated receive_decode_us={}",
                frame.frame_id(),
                frame.pixel_format(),
                frame.receive_decode_us()
            );
            return;
        };

        self.screen_modified = true;
        match present_remote_region_with(
            &mut self.opened,
            &mut self.markers,
            frame,
            dirty,
            self.submit_update,
        ) {
            Ok(submitted) => {
                self.remote_frame_cache.replace(frame, cached_pixels);
                eprintln!(
                    "mxcfb frame submitted id={} format={:?} stride={} bytes={} marker={} region=({},{} {}x{}) cache=updated receive_decode_us={}",
                    frame.frame_id(),
                    frame.pixel_format(),
                    frame.stride(),
                    frame.pixels().len(),
                    submitted.marker,
                    submitted.region.x,
                    submitted.region.y,
                    submitted.region.width,
                    submitted.region.height,
                    frame.receive_decode_us()
                );
            }
            Err(error) => eprintln!(
                "mxcfb frame failed id={} format={:?}: {error:#}",
                frame.frame_id(),
                frame.pixel_format()
            ),
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
            cached.pixel_format(),
            cached.stride(),
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
        self.screen_modified = true;
        match present_remote_with(
            &mut self.opened,
            &mut self.markers,
            frame,
            self.submit_update,
        ) {
            Ok(submitted) => eprintln!(
                "mxcfb frame restored id={} format={:?} cause={cause:?} marker={} region=({},{} {}x{})",
                frame.frame_id(),
                frame.pixel_format(),
                submitted.marker,
                submitted.region.x,
                submitted.region.y,
                submitted.region.width,
                submitted.region.height
            ),
            Err(error) => eprintln!(
                "mxcfb frame restore failed id={} format={:?} cause={cause:?}: {error:#}",
                frame.frame_id(),
                frame.pixel_format()
            ),
        }
    }

    fn draw_system_ui(&mut self, connected: bool) -> Result<()> {
        self.screen_modified = true;
        let submitted = present_system_ui_with(
            &mut self.opened,
            &mut self.markers,
            connected,
            self.submit_update,
        )?;
        self.last_connected = Some(connected);
        eprintln!(
            "mxcfb local system UI submitted connected={connected} marker={} region=({},{} {}x{})",
            submitted.marker,
            submitted.region.x,
            submitted.region.y,
            submitted.region.width,
            submitted.region.height
        );
        Ok(())
    }

    fn maintain(&mut self) -> Result<()> {
        let Some(refresh_at) = self.startup_refresh_at else {
            return Ok(());
        };
        if Instant::now() < refresh_at {
            return Ok(());
        }

        // Consume the deadline before touching the framebuffer. A failed
        // reassertion exits the event loop and restores the startup snapshot;
        // it must not be retried implicitly from Drop.
        self.startup_refresh_at = None;
        let content = if self.present_cached_remote("startup_reassert")? {
            "cached_frame"
        } else {
            self.clear_remote_viewport()
                .context("reassert white MXCFB startup viewport")?;
            "white"
        };
        if let Some(connected) = self.last_connected {
            self.draw_system_ui(connected)
                .context("reassert MXCFB startup system UI")?;
        }
        eprintln!(
            "mxcfb startup reasserted content={content} system_ui={}",
            self.last_connected.is_some()
        );
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        self.restore_initial_screen()
    }
}

impl Drop for MxcfbDisplayBackend {
    fn drop(&mut self) {
        if let Err(error) = self.restore_initial_screen() {
            eprintln!("mxcfb cleanup failed during drop: {error:#}");
        }
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
            accept_wait,
        )
        .unwrap()
    }

    fn accept_update(_: &File, _: &mut UpdateDataMtk, _: u32, _: u32) -> Result<()> {
        Ok(())
    }

    fn reject_update(_: &File, _: &mut UpdateDataMtk, _: u32, _: u32) -> Result<()> {
        anyhow::bail!("simulated update rejection")
    }

    fn accept_wait(_: &File, _: u32) -> Result<()> {
        Ok(())
    }

    #[test]
    fn cache_changes_only_after_accepted_remote_update() {
        let mut backend = fake_backend(accept_update);
        assert!(
            backend
                .pixel_formats()
                .supports(paper_protocol::PixelFormat::Mono1)
        );
        assert!(
            backend
                .pixel_formats()
                .supports(paper_protocol::PixelFormat::Gray8)
        );
        let first_pixels = [0x80; 8];
        let first = RemoteFrame::new(
            1,
            64,
            1,
            paper_protocol::PixelFormat::Mono1,
            8,
            &first_pixels,
            0,
        )
        .unwrap();
        backend.display_remote_frame(first);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 1);

        let gray8_pixels = [128; 64];
        let gray8 = RemoteFrame::new(
            2,
            64,
            1,
            paper_protocol::PixelFormat::Gray8,
            64,
            &gray8_pixels,
            0,
        )
        .unwrap();
        backend.submit_update = reject_update;
        backend.display_remote_frame(gray8);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 1);
        assert_eq!(
            backend.remote_frame_cache.current().unwrap().payload(),
            &first_pixels
        );

        backend.submit_update = accept_update;
        let gray8_retry = RemoteFrame::new(
            3,
            64,
            1,
            paper_protocol::PixelFormat::Gray8,
            64,
            &gray8_pixels,
            0,
        )
        .unwrap();
        backend.display_remote_frame(gray8_retry);
        let cached = backend.remote_frame_cache.current().unwrap();
        assert_eq!(cached.frame_id(), 3);
        assert_eq!(cached.pixel_format(), paper_protocol::PixelFormat::Gray8);
        assert_eq!(cached.payload(), &gray8_pixels);

        let wrong_pixels = [0x80, 0x00];
        let wrong = RemoteFrame::new(
            4,
            9,
            1,
            paper_protocol::PixelFormat::Mono1,
            2,
            &wrong_pixels,
            0,
        )
        .unwrap();
        backend.display_remote_frame(wrong);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 3);

        backend.submit_update = reject_update;
        let second_pixels = [0x00; 8];
        let second = RemoteFrame::new(
            5,
            64,
            1,
            paper_protocol::PixelFormat::Mono1,
            8,
            &second_pixels,
            0,
        )
        .unwrap();
        backend.display_remote_frame(second);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 3);
        assert_eq!(
            backend.remote_frame_cache.current().unwrap().payload(),
            &gray8_pixels
        );
        backend.redraw_cached_frame(RedrawCause::SurfaceDamage);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 3);
    }

    #[test]
    fn fixed_geometry_and_local_exit_failures_are_explicit() {
        let mut backend = fake_backend(accept_update);
        assert_eq!(backend.dimensions(), (64, 73));
        assert_eq!(backend.set_dimensions((64, 73)).unwrap(), None);
        assert!(backend.set_dimensions((65, 73)).is_err());
        backend.draw_system_ui(false).unwrap();
        backend.submit_update = reject_update;
        assert!(backend.draw_system_ui(true).is_err());
    }

    #[test]
    fn shutdown_restores_the_initial_framebuffer_snapshot() {
        let mut backend = fake_backend(accept_update);
        let initial = backend.opened.mapping.snapshot_visible().unwrap();
        backend.draw_system_ui(false).unwrap();
        assert_ne!(backend.opened.mapping.snapshot_visible().unwrap(), initial);
        backend.shutdown().unwrap();
        assert_eq!(backend.opened.mapping.snapshot_visible().unwrap(), initial);
        assert!(!backend.screen_modified);
        assert!(backend.initial_screen.is_none());
        backend.shutdown().unwrap();
    }

    #[test]
    fn startup_clear_whitens_only_the_remote_viewport() {
        let mut backend = fake_backend(accept_update);
        backend.clear_remote_viewport().unwrap();
        let pixels = backend.opened.mapping.snapshot_visible().unwrap();
        assert!(pixels[..64].iter().all(|&pixel| pixel == WHITE));
        assert!(pixels[64..].iter().all(|&pixel| pixel == 0));
        assert!(backend.screen_modified);
        backend.shutdown().unwrap();
    }

    #[test]
    fn startup_maintenance_reasserts_white_viewport_once() {
        let mut backend = fake_backend(accept_update);
        backend.clear_remote_viewport().unwrap();
        backend.draw_system_ui(false).unwrap();
        let expected = backend.opened.mapping.snapshot_visible().unwrap();
        let viewport = backend.opened.info.screen.remote_viewport;
        let spec = backend.opened.info.spec;
        backend
            .opened
            .mapping
            .fill_solid_region(viewport, 0, spec)
            .unwrap();

        backend.startup_refresh_at = Some(Instant::now());
        backend.maintain().unwrap();
        assert_eq!(backend.opened.mapping.snapshot_visible().unwrap(), expected);
        assert!(backend.startup_refresh_at.is_none());

        backend
            .opened
            .mapping
            .fill_solid_region(viewport, 0, spec)
            .unwrap();
        backend.maintain().unwrap();
        let after_second_maintenance = backend.opened.mapping.snapshot_visible().unwrap();
        assert!(
            after_second_maintenance[..64]
                .iter()
                .all(|&pixel| pixel == 0)
        );
        backend.shutdown().unwrap();
    }

    #[test]
    fn startup_maintenance_reasserts_cached_frame_and_system_ui() {
        let mut backend = fake_backend(accept_update);
        let pixels = [0; 8];
        let frame =
            RemoteFrame::new(9, 64, 1, paper_protocol::PixelFormat::Mono1, 8, &pixels, 0).unwrap();
        backend.display_remote_frame(frame);
        backend.draw_system_ui(true).unwrap();
        let expected = backend.opened.mapping.snapshot_visible().unwrap();
        let viewport = backend.opened.info.screen.remote_viewport;
        let spec = backend.opened.info.spec;
        backend
            .opened
            .mapping
            .fill_solid_region(viewport, WHITE, spec)
            .unwrap();

        backend.startup_refresh_at = Some(Instant::now());
        backend.maintain().unwrap();
        assert_eq!(backend.opened.mapping.snapshot_visible().unwrap(), expected);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 9);
        assert_eq!(backend.last_connected, Some(true));
        assert!(backend.startup_refresh_at.is_none());
        backend.shutdown().unwrap();
    }

    #[test]
    fn failed_startup_maintenance_is_not_retried() {
        let mut backend = fake_backend(accept_update);
        backend.startup_refresh_at = Some(Instant::now());
        backend.submit_update = reject_update;
        assert!(backend.maintain().is_err());
        assert!(backend.startup_refresh_at.is_none());

        backend.submit_update = accept_update;
        backend.shutdown().unwrap();
    }
}
