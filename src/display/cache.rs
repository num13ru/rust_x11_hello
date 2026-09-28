use paper_protocol::PixelFormat;

use super::RemoteFrame;

/// Payload from the most recent remote frame that a display backend presented
/// successfully.
///
/// The payload is deliberately generic: an X11 backend may retain a prepared
/// bitmap while another backend may retain a different backend-private value.
pub(crate) struct CachedRemoteFrame<T> {
    frame_id: u64,
    width: u16,
    height: u16,
    pixel_format: PixelFormat,
    stride: usize,
    payload: T,
}

impl<T> CachedRemoteFrame<T> {
    pub(crate) fn frame_id(&self) -> u64 {
        self.frame_id
    }

    pub(crate) fn dimensions(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub(crate) fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    pub(crate) fn stride(&self) -> usize {
        self.stride
    }

    pub(crate) fn payload(&self) -> &T {
        &self.payload
    }
}

/// Cache whose replacement remains an explicit post-presentation decision.
///
/// This type does not decide whether presentation succeeded. A display backend
/// must call [`Self::replace`] only after it has successfully presented the
/// corresponding frame.
pub(crate) struct RemoteFrameCache<T> {
    current: Option<CachedRemoteFrame<T>>,
}

impl<T> Default for RemoteFrameCache<T> {
    fn default() -> Self {
        Self { current: None }
    }
}

impl<T> RemoteFrameCache<T> {
    pub(crate) fn current(&self) -> Option<&CachedRemoteFrame<T>> {
        self.current.as_ref()
    }

    pub(crate) fn replace(&mut self, frame: RemoteFrame<'_>, payload: T) {
        self.current = Some(CachedRemoteFrame {
            frame_id: frame.frame_id(),
            width: frame.width(),
            height: frame.height(),
            pixel_format: frame.pixel_format(),
            stride: frame.stride(),
            payload,
        });
    }

    pub(crate) fn invalidate_mismatched(
        &mut self,
        viewport: (u16, u16),
    ) -> Option<CachedRemoteFrame<T>> {
        let mismatched = self
            .current
            .as_ref()
            .is_some_and(|frame| frame.dimensions() != viewport);
        mismatched.then(|| self.current.take().expect("mismatched frame exists"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_and_viewport_invalidation_preserve_cache_policy() {
        let mut cache = RemoteFrameCache::default();
        assert!(cache.current().is_none());

        let first_pixels = [0xaa, 0x80];
        let first = RemoteFrame::new(1, 9, 1, PixelFormat::Mono1, 2, &first_pixels, 0)
            .expect("valid Mono1 frame");
        cache.replace(first, first_pixels.to_vec());

        let second_pixels = [0, 64, 128, 255];
        let second = RemoteFrame::new(2, 2, 2, PixelFormat::Gray8, 2, &second_pixels, 0)
            .expect("valid Gray8 frame");
        cache.replace(second, second_pixels.to_vec());
        let current = cache.current().expect("replacement is cached");
        assert_eq!(current.frame_id(), 2);
        assert_eq!(current.dimensions(), (2, 2));
        assert_eq!(current.pixel_format(), PixelFormat::Gray8);
        assert_eq!(current.stride(), 2);
        assert_eq!(current.payload(), &second_pixels);

        let redrawn = RemoteFrame::new(
            current.frame_id(),
            current.dimensions().0,
            current.dimensions().1,
            current.pixel_format(),
            current.stride(),
            current.payload(),
            0,
        )
        .expect("cached Gray8 metadata reconstructs the same format");
        assert_eq!(redrawn.pixel_format(), PixelFormat::Gray8);
        assert_eq!(redrawn.stride(), 2);
        assert_eq!(redrawn.pixels(), &second_pixels);

        assert!(cache.invalidate_mismatched((2, 2)).is_none());
        assert_eq!(
            cache.current().expect("matching frame remains").frame_id(),
            2
        );

        let invalidated = cache
            .invalidate_mismatched((2, 3))
            .expect("mismatching frame is removed");
        assert_eq!(invalidated.frame_id(), 2);
        assert!(cache.current().is_none());
    }
}
