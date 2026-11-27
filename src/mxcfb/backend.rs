//! Native MXCFB display backend.

use std::env;
use std::fs::File;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::display::{RemoteFrame, RemoteFrameCache};

use super::controller;
use super::opened::OpenedFramebuffer;
use super::pixels::prepare_remote;
use super::present::{
    present_prepared_remote, present_remote_with_waveform, present_system_ui_with,
};
use super::update_abi::UpdateDataMtk;
use super::update_marker::UpdateMarkerSequence;
use super::update_request::{MxcfbWaveform, next_full_screen_gc16_request, next_remote_request};
use super::waveform_policy::{PreviousPixels, plan_updates};

type SubmitUpdate = fn(&File, &mut UpdateDataMtk, u32, u32) -> Result<()>;
type WaitUpdate = fn(&File, u32) -> Result<()>;
const WHITE: u8 = 0xff;
const STARTUP_REASSERT_DELAY: Duration = Duration::from_millis(750);
const WAVEFORM_ENV: &str = "PAPERPAD_MXCFB_WAVEFORM";

pub(crate) struct MxcfbDisplayBackend {
    opened: OpenedFramebuffer,
    markers: UpdateMarkerSequence,
    remote_frame_cache: RemoteFrameCache<Vec<u8>>,
    submit_update: SubmitUpdate,
    wait_update: WaitUpdate,
    remote_waveform: MxcfbWaveform,
    remote_pixels_known: bool,
    last_remote_update: Option<(MxcfbWaveform, u32)>,
    initial_screen: Option<Vec<u8>>,
    screen_modified: bool,
    startup_refresh_at: Option<Instant>,
    last_connected: Option<bool>,
}

impl MxcfbDisplayBackend {
    pub(crate) fn open() -> Result<Self> {
        let remote_waveform = waveform_from_env()?;
        let opened = OpenedFramebuffer::open().context("initialize MXCFB display backend")?;
        let mut backend = Self::from_opened(
            opened,
            controller::submit_update,
            controller::wait_for_update_complete,
            remote_waveform,
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
        remote_waveform: MxcfbWaveform,
    ) -> Result<Self> {
        let spec = opened.info.spec;
        let initial_screen = opened
            .mapping
            .snapshot_visible()
            .context("snapshot framebuffer before MXCFB PaperPad")?;
        eprintln!(
            "display backend=mxcfb width={} height={} bpp={} line_length={} visual={} xoffset={} yoffset={} snapshot_bytes={} remote_waveform={}",
            spec.visible_width,
            spec.visible_height,
            spec.bits_per_pixel,
            spec.line_length,
            spec.visual,
            spec.xoffset,
            spec.yoffset,
            initial_screen.len(),
            remote_waveform.name()
        );
        Ok(Self {
            opened,
            markers: UpdateMarkerSequence::for_process(),
            remote_frame_cache: RemoteFrameCache::default(),
            submit_update,
            wait_update,
            remote_waveform,
            initial_screen: Some(initial_screen),
            remote_pixels_known: false,
            last_remote_update: None,
            screen_modified: false,
            startup_refresh_at: None,
            last_connected: None,
        })
    }

    fn restore_initial_screen(&mut self) -> Result<()> {
        if !self.screen_modified {
            return Ok(());
        }
        if let Err(error) = self.wait_on_waveform_change(MxcfbWaveform::Gc16) {
            eprintln!("mxcfb cleanup could not wait for previous remote update: {error:#}");
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
        self.last_remote_update = None;
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
        self.remote_pixels_known = true;
        self.last_remote_update = Some((MxcfbWaveform::Gc16, marker));
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
        // Startup reassertion recovers from a possible write by the Kindle UI.
        // The prior panel pixels are unknown, so use GC16.
        let waveform = MxcfbWaveform::Gc16;
        if self.remote_frame_cache.current().is_none() {
            return Ok(false);
        }
        self.wait_on_waveform_change(waveform)?;
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
        let submitted = present_remote_with_waveform(
            &mut self.opened,
            &mut self.markers,
            frame,
            waveform,
            self.submit_update,
        )?;
        self.remote_pixels_known = true;
        self.last_remote_update = Some((waveform, submitted.marker));
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

    fn wait_on_waveform_change(&self, waveform: MxcfbWaveform) -> Result<()> {
        if let Some((previous, marker)) = self.last_remote_update
            && previous != waveform
        {
            let started = Instant::now();
            (self.wait_update)(&self.opened.file, marker)
                .context("wait for previous MXCFB waveform")?;
            eprintln!(
                "mxcfb waveform transition from={} to={} marker={marker} wait_us={}",
                previous.name(),
                waveform.name(),
                started.elapsed().as_micros()
            );
        }
        Ok(())
    }
}

impl MxcfbDisplayBackend {
    pub(crate) fn dimensions(&self) -> (u16, u16) {
        self.opened.info.screen.physical_size()
    }

    pub(crate) fn pixel_formats(&self) -> paper_protocol::V2PixelFormats {
        paper_protocol::V2PixelFormats::MONO1.with(paper_protocol::PixelFormat::Gray8)
    }

    pub(crate) fn display_remote_frame(&mut self, frame: RemoteFrame<'_>) {
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

        let previous = if !self.remote_pixels_known {
            PreviousPixels::Unknown
        } else if let Some(cached) = self.remote_frame_cache.current() {
            let (width, height) = cached.dimensions();
            match RemoteFrame::new(
                cached.frame_id(),
                width,
                height,
                cached.pixel_format(),
                cached.stride(),
                cached.payload(),
                0,
            ) {
                Ok(cached) if (width, height) == (frame.width(), frame.height()) => {
                    PreviousPixels::Frame(cached)
                }
                _ => PreviousPixels::Unknown,
            }
        } else {
            PreviousPixels::White
        };
        let updates = plan_updates(previous, frame, self.remote_waveform);

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

        if updates.is_empty() {
            eprintln!(
                "mxcfb frame unchanged id={} format={:?} cache=updated receive_decode_us={}",
                frame.frame_id(),
                frame.pixel_format(),
                frame.receive_decode_us()
            );
        } else {
            let info = &self.opened.info;
            let prepared = match prepare_remote(frame, info.screen, info.spec, info.mapping_len) {
                Ok(prepared) => prepared,
                Err(error) => {
                    eprintln!(
                        "mxcfb frame preparation failed id={}: {error:#}",
                        frame.frame_id()
                    );
                    return;
                }
            };
            self.screen_modified = true;
            for update in updates {
                let started = Instant::now();
                let result = self
                    .wait_on_waveform_change(update.waveform)
                    .and_then(|()| {
                        present_prepared_remote(
                            &mut self.opened,
                            &mut self.markers,
                            &prepared,
                            update.region,
                            update.waveform,
                            self.submit_update,
                        )
                    });
                let submitted = match result {
                    Ok(submitted) => submitted,
                    Err(error) => {
                        // A partial presentation or a rejected request leaves the panel
                        // unsuitable for a diff against the last accepted frame.
                        self.remote_pixels_known = false;
                        eprintln!(
                            "mxcfb frame failed id={} format={:?}: {error:#}",
                            frame.frame_id(),
                            frame.pixel_format()
                        );
                        return;
                    }
                };
                self.last_remote_update = Some((update.waveform, submitted.marker));
                eprintln!(
                    "mxcfb frame submitted id={} format={:?} stride={} bytes={} marker={} region=({},{} {}x{}) waveform={} receive_decode_us={} present_us={}",
                    frame.frame_id(),
                    frame.pixel_format(),
                    frame.stride(),
                    frame.pixels().len(),
                    submitted.marker,
                    submitted.region.x,
                    submitted.region.y,
                    submitted.region.width,
                    submitted.region.height,
                    update.waveform.name(),
                    frame.receive_decode_us(),
                    started.elapsed().as_micros()
                );
            }
        }
        self.remote_frame_cache.replace(frame, cached_pixels);
        self.remote_pixels_known = true;
    }

    pub(crate) fn draw_system_ui(&mut self, connected: bool) -> Result<()> {
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

    pub(crate) fn next_maintenance_deadline(&self) -> Option<Instant> {
        self.startup_refresh_at
    }

    pub(crate) fn maintain(&mut self) -> Result<()> {
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

    pub(crate) fn shutdown(&mut self) -> Result<()> {
        self.restore_initial_screen()
    }
}

fn waveform_from_env() -> Result<MxcfbWaveform> {
    match env::var(WAVEFORM_ENV) {
        Ok(value) => parse_waveform(Some(&value)),
        Err(env::VarError::NotPresent) => parse_waveform(None),
        Err(env::VarError::NotUnicode(_)) => bail!("{WAVEFORM_ENV} is not valid Unicode"),
    }
}

fn parse_waveform(value: Option<&str>) -> Result<MxcfbWaveform> {
    match value {
        None => Ok(MxcfbWaveform::Du),
        Some(name) => MxcfbWaveform::from_name(name).with_context(|| {
            format!("invalid {WAVEFORM_ENV}={name:?}; expected gc16, du, a2, or du4")
        }),
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

    #[test]
    fn runtime_waveform_defaults_to_du_and_accepts_supported_modes() {
        assert_eq!(parse_waveform(None).unwrap(), MxcfbWaveform::Du);
        for (name, expected) in [
            ("gc16", MxcfbWaveform::Gc16),
            ("du", MxcfbWaveform::Du),
            ("a2", MxcfbWaveform::A2),
            ("du4", MxcfbWaveform::Du4),
        ] {
            assert_eq!(parse_waveform(Some(name)).unwrap(), expected);
        }

        let error = parse_waveform(Some("auto")).unwrap_err();
        assert!(format!("{error:#}").contains(WAVEFORM_ENV));
    }

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
            MxcfbWaveform::Gc16,
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
    fn rejected_fast_update_after_grayscale_recovers_with_full_gc16() {
        use paper_protocol::PixelFormat;

        let mut backend = fake_backend(accept_update);
        backend.remote_waveform = MxcfbWaveform::Du;
        backend.clear_remote_viewport().unwrap();
        let white = [255; 64];
        backend.display_remote_frame(
            RemoteFrame::new(1, 64, 1, PixelFormat::Gray8, 64, &white, 0).unwrap(),
        );
        let mut mixed = white;
        mixed[2] = 80;
        mixed[60] = 0;
        backend.submit_update = |_, request, _, _| {
            if request.update_region.left == 60 {
                anyhow::bail!("simulated fast update rejection");
            }
            assert_eq!(request.update_region.left, 2);
            assert_eq!(request.update_region.width, 1);
            Ok(())
        };
        backend.display_remote_frame(
            RemoteFrame::new(2, 64, 1, PixelFormat::Gray8, 64, &mixed, 0).unwrap(),
        );
        assert!(!backend.remote_pixels_known);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 1);
        let gray_marker = backend.last_remote_update.unwrap();
        assert_eq!(gray_marker.0, MxcfbWaveform::Gc16);

        backend.submit_update = |_, request, _, _| {
            assert_eq!(request.update_region.left, 0);
            assert_eq!(request.update_region.width, 64);
            Ok(())
        };
        backend.display_remote_frame(
            RemoteFrame::new(3, 64, 1, PixelFormat::Gray8, 64, &white, 0).unwrap(),
        );
        assert!(backend.remote_pixels_known);
        assert_eq!(backend.remote_frame_cache.current().unwrap().frame_id(), 3);
        backend.submit_update = accept_update;
        backend.shutdown().unwrap();
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
    }

    #[test]
    fn fixed_geometry_and_local_exit_failures_are_explicit() {
        let mut backend = fake_backend(accept_update);
        assert_eq!(backend.dimensions(), (64, 73));
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

        let deadline = Instant::now();
        backend.startup_refresh_at = Some(deadline);
        assert_eq!(backend.next_maintenance_deadline(), Some(deadline));
        backend.maintain().unwrap();
        assert_eq!(backend.opened.mapping.snapshot_visible().unwrap(), expected);
        assert_eq!(backend.next_maintenance_deadline(), None);

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
