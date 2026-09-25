/// Payload from the most recent remote frame that a display backend presented
/// successfully.
///
/// The payload is deliberately generic: an X11 backend may retain a prepared
/// bitmap while another backend may retain a different backend-private value.
pub(crate) struct CachedRemoteFrame<T> {
    frame_id: u64,
    width: u16,
    height: u16,
    payload: T,
}

impl<T> CachedRemoteFrame<T> {
    pub(crate) fn frame_id(&self) -> u64 {
        self.frame_id
    }

    pub(crate) fn dimensions(&self) -> (u16, u16) {
        (self.width, self.height)
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

    pub(crate) fn replace(&mut self, frame_id: u64, width: u16, height: u16, payload: T) {
        self.current = Some(CachedRemoteFrame {
            frame_id,
            width,
            height,
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

        cache.replace(1, 9, 2, "first");
        cache.replace(2, 17, 4, "second");
        let current = cache.current().expect("replacement is cached");
        assert_eq!(current.frame_id(), 2);
        assert_eq!(current.dimensions(), (17, 4));
        assert_eq!(*current.payload(), "second");

        assert!(cache.invalidate_mismatched((17, 4)).is_none());
        assert_eq!(
            cache.current().expect("matching frame remains").frame_id(),
            2
        );

        let invalidated = cache
            .invalidate_mismatched((17, 5))
            .expect("mismatching frame is removed");
        assert_eq!(invalidated.frame_id(), 2);
        assert!(cache.current().is_none());
    }
}
