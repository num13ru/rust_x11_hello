use crate::viewport::SourcePoint;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PointerContact {
    last_point: Option<SourcePoint>,
}

impl PointerContact {
    pub fn is_active(self) -> bool {
        self.last_point.is_some()
    }

    pub fn begin(&mut self, point: SourcePoint) -> bool {
        if self.is_active() {
            return false;
        }
        self.last_point = Some(point);
        true
    }

    pub fn update(&mut self, point: Option<SourcePoint>) {
        if self.is_active()
            && let Some(point) = point
        {
            self.last_point = Some(point);
        }
    }

    pub fn end(&mut self, point: Option<SourcePoint>) -> Option<SourcePoint> {
        let last_point = self.last_point.take()?;
        Some(point.unwrap_or(last_point))
    }

    pub fn cancel(&mut self) -> Option<SourcePoint> {
        self.end(None)
    }

    pub fn clear(&mut self) -> bool {
        self.last_point.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: SourcePoint = SourcePoint { x: 10, y: 20 };
    const SECOND: SourcePoint = SourcePoint { x: 30, y: 40 };

    #[test]
    fn duplicate_down_is_suppressed_until_contact_ends() {
        let mut contact = PointerContact::default();
        assert!(contact.begin(FIRST));
        assert!(!contact.begin(SECOND));
        assert_eq!(contact.end(Some(SECOND)), Some(SECOND));
        assert_eq!(contact.end(Some(FIRST)), None);
        assert!(contact.begin(SECOND));
    }

    #[test]
    fn outside_release_and_cancel_use_last_valid_point_once() {
        let mut contact = PointerContact::default();
        assert!(contact.begin(FIRST));
        contact.update(None);
        contact.update(Some(SECOND));
        contact.update(None);
        assert_eq!(contact.end(None), Some(SECOND));
        assert_eq!(contact.cancel(), None);

        assert!(contact.begin(FIRST));
        assert_eq!(contact.cancel(), Some(FIRST));
        assert_eq!(contact.cancel(), None);
    }

    #[test]
    fn disconnect_clear_does_not_create_a_future_release() {
        let mut contact = PointerContact::default();
        assert!(contact.begin(FIRST));
        assert!(contact.clear());
        assert!(!contact.clear());
        assert_eq!(contact.end(Some(SECOND)), None);
        assert!(contact.begin(SECOND));
    }
}
