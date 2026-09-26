//! Validate Linux framebuffer metadata before constructing an MXCFB backend.
//! No device access occurs in this module.

use anyhow::{Context, Result, ensure};

use super::abi::{FbFixScreeninfo, FbVarScreeninfo};
use super::layout::{FramebufferGeometry, visible_mapping_len};
use super::pixels::FramebufferSpec;
use crate::ui::screen::{SYSTEM_UI_HEIGHT, ScreenLayout};

pub(super) struct FramebufferInfo {
    pub geometry: FramebufferGeometry,
    pub spec: FramebufferSpec,
    pub screen: ScreenLayout,
    pub mapping_len: usize,
}

impl FramebufferInfo {
    pub(super) fn from_kernel(fixed: &FbFixScreeninfo, variable: &FbVarScreeninfo) -> Result<Self> {
        let id_end = fixed
            .id
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(fixed.id.len());
        let id = &fixed.id[..id_end];
        ensure!(
            id == b"hwtcon_v2",
            "unsupported framebuffer driver {:?}",
            String::from_utf8_lossy(id)
        );
        ensure!(variable.rotate == 0, "rotated framebuffer is unsupported");
        ensure!(
            variable.xres <= i16::MAX as u32 && variable.yres <= i16::MAX as u32,
            "framebuffer dimensions exceed PaperPad physical coordinates"
        );

        let screen =
            ScreenLayout::new(u16::try_from(variable.xres)?, u16::try_from(variable.yres)?)
                .context("empty framebuffer screen")?;
        ensure!(
            screen.system_ui_region.height == SYSTEM_UI_HEIGHT && screen.remote_viewport.height > 0,
            "framebuffer is too short for remote content and local Exit strip"
        );

        let geometry = FramebufferGeometry {
            xres: variable.xres,
            yres: variable.yres,
            xres_virtual: variable.xres_virtual,
            yres_virtual: variable.yres_virtual,
            xoffset: variable.xoffset,
            yoffset: variable.yoffset,
            bits_per_pixel: variable.bits_per_pixel,
            line_length: fixed.line_length,
            smem_len: fixed.smem_len,
        };
        let mapping_len = visible_mapping_len(geometry).context("visible framebuffer bounds")?;
        let spec = FramebufferSpec {
            visible_width: variable.xres,
            visible_height: variable.yres,
            virtual_width: variable.xres_virtual,
            virtual_height: variable.yres_virtual,
            xoffset: variable.xoffset,
            yoffset: variable.yoffset,
            line_length: usize::try_from(fixed.line_length)?,
            memory_len: usize::try_from(fixed.smem_len)?,
            kind: fixed.kind,
            visual: fixed.visual,
            bits_per_pixel: variable.bits_per_pixel,
            grayscale: variable.grayscale,
            nonstd: variable.nonstd,
        };
        spec.validate_format()?;

        Ok(Self {
            geometry,
            spec,
            screen,
            mapping_len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pw6_info() -> (FbFixScreeninfo, FbVarScreeninfo) {
        let mut fixed = FbFixScreeninfo::default();
        fixed.id[..9].copy_from_slice(b"hwtcon_v2");
        fixed.smem_len = 4_314_624;
        fixed.kind = 0;
        fixed.visual = 1;
        fixed.line_length = 1272;

        let variable = FbVarScreeninfo {
            xres: 1272,
            yres: 1696,
            xres_virtual: 1272,
            yres_virtual: 3392,
            bits_per_pixel: 8,
            grayscale: 1,
            ..FbVarScreeninfo::default()
        };
        (fixed, variable)
    }

    #[test]
    fn accepts_probed_pw6_metadata() {
        let (fixed, variable) = pw6_info();
        let info = FramebufferInfo::from_kernel(&fixed, &variable).unwrap();
        assert_eq!(info.mapping_len, 2_157_312);
        assert_eq!(info.screen.remote_viewport.height, 1624);
        assert_eq!(info.screen.system_ui_region.height, 72);
        assert_eq!(info.spec.line_length, 1272);
        assert_eq!(info.spec.memory_len, 4_314_624);
        assert_eq!(info.geometry.yres_virtual, 3392);
    }

    #[test]
    fn rejects_unknown_driver_rotation_and_format() {
        let (mut fixed, variable) = pw6_info();
        fixed.id[0] = b'x';
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());

        let (fixed, mut variable) = pw6_info();
        variable.rotate = 1;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());

        let (mut fixed, variable) = pw6_info();
        fixed.visual = 0;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());

        let (fixed, mut variable) = pw6_info();
        variable.bits_per_pixel = 4;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());
    }

    #[test]
    fn rejects_unsafe_dimensions_and_memory_bounds() {
        let (fixed, mut variable) = pw6_info();
        variable.xres = i16::MAX as u32 + 1;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());

        let (fixed, mut variable) = pw6_info();
        variable.yres = 72;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());

        let (mut fixed, variable) = pw6_info();
        fixed.smem_len = 1024;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());

        let (fixed, mut variable) = pw6_info();
        variable.xoffset = 1;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());

        let (mut fixed, variable) = pw6_info();
        fixed.line_length = 1271;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());

        let (fixed, mut variable) = pw6_info();
        variable.xres = 0;
        assert!(FramebufferInfo::from_kernel(&fixed, &variable).is_err());
    }
}
