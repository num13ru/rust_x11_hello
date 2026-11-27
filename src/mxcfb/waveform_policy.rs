//! Select remote updates by source and destination pixel levels.

use paper_protocol::PixelFormat;

use crate::display::RemoteFrame;
use crate::ui::screen::ScreenRect;

use super::update_request::MxcfbWaveform;

// Bound work and driver submissions for arbitrarily fragmented remote frames.
const MAX_SPLIT_DEPTH: u8 = 5;

#[derive(Clone, Copy)]
pub(super) enum PreviousPixels<'a> {
    Unknown,
    White,
    Frame(RemoteFrame<'a>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RemoteUpdate {
    pub region: ScreenRect,
    pub waveform: MxcfbWaveform,
}

pub(super) fn plan_updates(
    previous: PreviousPixels<'_>,
    frame: RemoteFrame<'_>,
    fast: MxcfbWaveform,
) -> Vec<RemoteUpdate> {
    let viewport = ScreenRect {
        x: 0,
        y: 0,
        width: frame.width(),
        height: frame.height(),
    };
    let previous = match previous {
        PreviousPixels::Frame(frame) => Some(frame),
        PreviousPixels::White => None,
        PreviousPixels::Unknown => {
            return vec![RemoteUpdate {
                region: viewport,
                waveform: MxcfbWaveform::Gc16,
            }];
        }
    };
    let mut updates = Vec::new();
    plan_region(
        previous,
        frame,
        fast,
        viewport,
        MAX_SPLIT_DEPTH,
        &mut updates,
    );
    // Apply the grayscale updates together, with one barrier before the fast updates.
    updates.sort_by_key(|update| update.waveform != MxcfbWaveform::Gc16);
    updates
}

fn plan_region(
    previous: Option<RemoteFrame<'_>>,
    frame: RemoteFrame<'_>,
    fast: MxcfbWaveform,
    region: ScreenRect,
    depth: u8,
    updates: &mut Vec<RemoteUpdate>,
) {
    let mut dirty = None;
    let mut fast_dirty = false;
    let mut slow_dirty = false;
    for y in region.y..region.y + region.height {
        let before = Pixels::row(previous, y);
        let after = Pixels::row(Some(frame), y);
        if before == after {
            continue;
        }
        for x in region.x..region.x + region.width {
            let a = before.at(x);
            let b = after.at(x);
            if a != b {
                include(&mut dirty, x, y);
                if supports(fast, a) && supports(fast, b) {
                    fast_dirty = true;
                } else {
                    slow_dirty = true;
                }
            }
        }
    }
    let Some(dirty) = dirty else { return };
    if fast == MxcfbWaveform::Gc16 || !fast_dirty {
        updates.push(RemoteUpdate {
            region: dirty,
            waveform: MxcfbWaveform::Gc16,
        });
        return;
    }

    // Even unchanged gray pixels inside an update rectangle can be altered by DU
    // or A2. Check the entire rectangle, including pixels between changed glyphs.
    let mut incompatible = None;
    for y in dirty.y..dirty.y + dirty.height {
        let before = Pixels::row(previous, y);
        let after = Pixels::row(Some(frame), y);
        for x in dirty.x..dirty.x + dirty.width {
            if !supports(fast, before.at(x)) || !supports(fast, after.at(x)) {
                include(&mut incompatible, x, y);
            }
        }
    }
    let Some(incompatible) = incompatible else {
        updates.push(RemoteUpdate {
            region: dirty,
            waveform: fast,
        });
        return;
    };

    let split = if depth > 0 {
        split_around(dirty, incompatible).or_else(|| {
            // If unchanged grayscale encloses binary changes, try isolating the changes.
            if slow_dirty { None } else { bisect(dirty) }
        })
    } else {
        None
    };
    if let Some(parts) = split {
        for part in parts {
            plan_region(previous, frame, fast, part, depth - 1, updates);
        }
    } else {
        updates.push(RemoteUpdate {
            region: dirty,
            waveform: MxcfbWaveform::Gc16,
        });
    }
}

fn split_around(region: ScreenRect, inner: ScreenRect) -> Option<[ScreenRect; 2]> {
    if inner.y > region.y {
        return split_at(region, inner.y, false);
    }
    if inner.y + inner.height < region.y + region.height {
        return split_at(region, inner.y + inner.height, false);
    }
    if inner.x > region.x {
        return split_at(region, inner.x, true);
    }
    if inner.x + inner.width < region.x + region.width {
        return split_at(region, inner.x + inner.width, true);
    }
    None
}

fn bisect(region: ScreenRect) -> Option<[ScreenRect; 2]> {
    if region.width >= region.height {
        split_at(region, region.x + region.width / 2, true)
    } else {
        split_at(region, region.y + region.height / 2, false)
    }
}

fn split_at(region: ScreenRect, at: u16, vertical: bool) -> Option<[ScreenRect; 2]> {
    if vertical {
        let right = region.x + region.width;
        (region.x < at && at < right).then(|| {
            [
                ScreenRect {
                    width: at - region.x,
                    ..region
                },
                ScreenRect {
                    x: at,
                    width: right - at,
                    ..region
                },
            ]
        })
    } else {
        let bottom = region.y + region.height;
        (region.y < at && at < bottom).then(|| {
            [
                ScreenRect {
                    height: at - region.y,
                    ..region
                },
                ScreenRect {
                    y: at,
                    height: bottom - at,
                    ..region
                },
            ]
        })
    }
}

fn include(bounds: &mut Option<ScreenRect>, x: u16, y: u16) {
    *bounds = Some(match *bounds {
        None => ScreenRect {
            x,
            y,
            width: 1,
            height: 1,
        },
        Some(rect) => {
            let left = rect.x.min(x);
            ScreenRect {
                x: left,
                y: rect.y,
                width: (rect.x + rect.width).max(x + 1) - left,
                height: y + 1 - rect.y,
            }
        }
    });
}

fn supports(waveform: MxcfbWaveform, level: u8) -> bool {
    matches!(level, 0 | 255)
        || waveform == MxcfbWaveform::Gc16
        || (waveform == MxcfbWaveform::Du4 && matches!(level, 85 | 170))
}

#[derive(PartialEq)]
enum Pixels<'a> {
    Gray(&'a [u8]),
    Mono(&'a [u8]),
    White,
}

impl<'a> Pixels<'a> {
    fn row(frame: Option<RemoteFrame<'a>>, y: u16) -> Self {
        let Some(frame) = frame else {
            return Self::White;
        };
        let start = usize::from(y) * frame.stride();
        let bytes = &frame.pixels()[start..start + frame.stride()];
        match frame.pixel_format() {
            PixelFormat::Gray8 => Self::Gray(bytes),
            PixelFormat::Mono1 => Self::Mono(bytes),
        }
    }

    fn at(&self, x: u16) -> u8 {
        let x = usize::from(x);
        match self {
            Self::Gray(row) => row[x],
            Self::Mono(row) if row[x / 8] & (0x80 >> (x % 8)) != 0 => 0,
            Self::Mono(_) | Self::White => 255,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MxcfbWaveform::{A2, Du, Du4, Gc16};

    fn gray<'a>(width: u16, height: u16, bytes: &'a [u8]) -> RemoteFrame<'a> {
        RemoteFrame::new(
            1,
            width,
            height,
            PixelFormat::Gray8,
            usize::from(width),
            bytes,
            0,
        )
        .unwrap()
    }

    fn update(waveform: MxcfbWaveform, x: u16, y: u16, width: u16, height: u16) -> RemoteUpdate {
        RemoteUpdate {
            waveform,
            region: ScreenRect {
                x,
                y,
                width,
                height,
            },
        }
    }

    #[test]
    fn white_baseline_distinguishes_binary_changes_and_grayscale() {
        let pixels = [60, 160, 255, 0, 255, 0];
        let frame = gray(3, 2, &pixels);
        assert_eq!(
            plan_updates(PreviousPixels::White, frame, Du),
            [update(Gc16, 0, 0, 2, 1), update(Du, 0, 1, 3, 1),]
        );
        assert_eq!(
            plan_updates(PreviousPixels::Unknown, frame, Du),
            [update(Gc16, 0, 0, 3, 2),]
        );
        assert_eq!(
            plan_updates(PreviousPixels::White, frame, Gc16),
            [update(Gc16, 0, 0, 3, 2),]
        );
    }

    #[test]
    fn uses_previous_levels_even_when_replaced_with_mono1() {
        let before = gray(3, 1, &[60, 255, 0]);
        let after = RemoteFrame::new(2, 3, 1, PixelFormat::Mono1, 1, &[0x60], 0).unwrap();
        assert_eq!(
            plan_updates(PreviousPixels::Frame(before), after, Du),
            [update(Gc16, 0, 0, 1, 1), update(Du, 1, 0, 1, 1),]
        );
        assert!(
            plan_updates(PreviousPixels::Frame(after), gray(3, 1, &[255, 0, 0]), Du).is_empty()
        );
    }

    #[test]
    fn leaves_unchanged_gray_between_binary_changes_outside_updates() {
        let before = gray(5, 2, &[255, 75, 128, 90, 255, 255, 255, 255, 255, 255]);
        let after = gray(5, 2, &[0, 75, 128, 90, 0, 255, 255, 255, 255, 255]);
        assert_eq!(
            plan_updates(PreviousPixels::Frame(before), after, Du),
            [update(Du, 0, 0, 1, 1), update(Du, 4, 0, 1, 1),]
        );
    }

    #[test]
    fn fast_modes_respect_their_supported_levels_in_both_frames() {
        let before = gray(3, 1, &[85, 255, 100]);
        let after = gray(3, 1, &[170, 0, 0]);
        assert_eq!(
            plan_updates(PreviousPixels::Frame(before), after, Du4),
            [update(Gc16, 2, 0, 1, 1), update(Du4, 0, 0, 2, 1),]
        );
        assert_eq!(
            plan_updates(PreviousPixels::Frame(before), after, A2),
            [update(Gc16, 0, 0, 3, 1),]
        );
        assert_eq!(
            plan_updates(PreviousPixels::White, gray(3, 1, &[255, 0, 255]), A2),
            [update(A2, 1, 0, 1, 1),]
        );
    }
}
