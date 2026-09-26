//! Bounds checks for a read-only mapping of the visible framebuffer page.
//! No device access or pixel-format conversion happens here.

use anyhow::{Context, Result, ensure};

#[derive(Clone, Copy, Debug)]
pub(super) struct FramebufferGeometry {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub line_length: u32,
    pub smem_len: u32,
}

pub(super) fn visible_mapping_len(geometry: FramebufferGeometry) -> Result<usize> {
    ensure!(
        geometry.xres > 0 && geometry.yres > 0 && geometry.bits_per_pixel > 0,
        "framebuffer has empty resolution or zero bits per pixel"
    );
    let visible_right = geometry
        .xoffset
        .checked_add(geometry.xres)
        .context("framebuffer horizontal offset overflow")?;
    let visible_bottom = geometry
        .yoffset
        .checked_add(geometry.yres)
        .context("framebuffer vertical offset overflow")?;
    ensure!(
        visible_right <= geometry.xres_virtual && visible_bottom <= geometry.yres_virtual,
        "visible framebuffer exceeds virtual dimensions"
    );

    let row_bits = u64::from(geometry.xres_virtual)
        .checked_mul(u64::from(geometry.bits_per_pixel))
        .context("virtual framebuffer row bit count overflow")?;
    let row_bytes = row_bits
        .checked_add(7)
        .context("virtual framebuffer row byte count overflow")?
        / 8;
    ensure!(
        u64::from(geometry.line_length) >= row_bytes,
        "framebuffer line length is shorter than the virtual pixel row"
    );

    let line_length = usize::try_from(geometry.line_length)?;
    let virtual_height = usize::try_from(geometry.yres_virtual)?;
    let visible_bottom = usize::try_from(visible_bottom)?;
    let smem_len = usize::try_from(geometry.smem_len)?;
    let virtual_bytes = line_length
        .checked_mul(virtual_height)
        .context("virtual framebuffer byte count overflow")?;
    ensure!(
        virtual_bytes <= smem_len,
        "virtual framebuffer exceeds reported memory length"
    );
    let visible_bytes = line_length
        .checked_mul(visible_bottom)
        .context("visible framebuffer byte count overflow")?;
    ensure!(visible_bytes > 0, "empty visible framebuffer mapping");
    Ok(visible_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probed_geometry() -> FramebufferGeometry {
        FramebufferGeometry {
            xres: 1272,
            yres: 1696,
            xres_virtual: 1272,
            yres_virtual: 3392,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 8,
            line_length: 1272,
            smem_len: 4_314_624,
        }
    }

    #[test]
    fn probed_geometry_maps_only_the_visible_page() {
        assert_eq!(visible_mapping_len(probed_geometry()).unwrap(), 2_157_312);
    }

    #[test]
    fn vertical_offset_and_padded_rows_extend_the_required_mapping() {
        let geometry = FramebufferGeometry {
            xres: 9,
            yres: 2,
            xres_virtual: 12,
            yres_virtual: 6,
            xoffset: 2,
            yoffset: 3,
            bits_per_pixel: 8,
            line_length: 16,
            smem_len: 96,
        };
        assert_eq!(visible_mapping_len(geometry).unwrap(), 80);
    }

    #[test]
    fn invalid_geometry_and_overflow_are_rejected() {
        let base = probed_geometry();
        for bad in [
            FramebufferGeometry { xres: 0, ..base },
            FramebufferGeometry {
                bits_per_pixel: 0,
                ..base
            },
            FramebufferGeometry {
                xoffset: u32::MAX,
                ..base
            },
            FramebufferGeometry {
                yoffset: u32::MAX,
                ..base
            },
            FramebufferGeometry {
                xres_virtual: 1271,
                ..base
            },
            FramebufferGeometry {
                yres_virtual: 1695,
                ..base
            },
            FramebufferGeometry {
                line_length: 1271,
                ..base
            },
            FramebufferGeometry {
                smem_len: 2_157_311,
                ..base
            },
            FramebufferGeometry {
                line_length: u32::MAX,
                ..base
            },
        ] {
            assert!(visible_mapping_len(bad).is_err(), "accepted {bad:?}");
        }
    }
}
