//! Toast notifications (bottom-left): a self-contained notification
//! mechanism for launcher errors, game start/stop and anything else that
//! needs a non-blocking heads-up.
//!
//! Lifecycle: a toast appears at the bottom-left corner and holds for
//! [`TOAST_HOLD_SECS`]. Left-clicking it *pins* it (it stops aging and
//! opens the logs screen); a pinned toast can be dismissed with the ✕ in
//! its top-right corner at any moment. An unpinned toast ages out and
//! slides right off-screen.

/// Seconds an unpinned toast stays fully visible before sliding away.
pub const TOAST_HOLD_SECS: f32 = 5.0;

/// Seconds an unpinned toast takes to slide right off-screen.
pub const TOAST_SLIDE_SECS: f32 = 1.2;

/// What kind of notification a toast carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// Launcher error: rendered with the material yellow warning triangle,
    /// shows the error text/code and the last log lines on click.
    Error,
    /// Informational notification (game started/stopped etc.).
    Info,
}

/// One toast notification.
#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub title: String,
    /// Optional error code / short tag shown after the title.
    pub code: Option<String>,
    /// Optional multi-line detail (for errors: the last log lines).
    pub detail: Option<String>,
    /// Seconds since the toast was created.
    age: f32,
    /// Pinned by a left-click: stops aging until dismissed.
    pinned: bool,
}

impl Toast {
    pub fn error(title: impl Into<String>, code: Option<String>, detail: Option<String>) -> Self {
        Toast {
            kind: ToastKind::Error,
            title: title.into(),
            code,
            detail,
            age: 0.0,
            pinned: false,
        }
    }

    pub fn info(title: impl Into<String>) -> Self {
        Toast {
            kind: ToastKind::Info,
            title: title.into(),
            code: None,
            detail: None,
            age: 0.0,
            pinned: false,
        }
    }

    #[cfg(test)]
    pub fn age(&self) -> f32 {
        self.age
    }

    #[cfg(test)]
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// Left-click: pin the toast (it stops aging until dismissed).
    pub fn pin(&mut self) {
        self.pinned = true;
    }

    /// Advance the age; returns `true` when an unpinned toast has fully
    /// slid away and should be removed.
    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.pinned {
            self.age += dt;
        }
        !self.pinned && self.age >= TOAST_HOLD_SECS + TOAST_SLIDE_SECS
    }

    /// Horizontal offset (pixels) while sliding out; 0 while holding.
    pub fn slide_offset(&self) -> f32 {
        if self.pinned || self.age <= TOAST_HOLD_SECS {
            0.0
        } else {
            let t = (self.age - TOAST_HOLD_SECS) / TOAST_SLIDE_SECS;
            t * t * 600.0 // ease-in slide
        }
    }
}

/// The toast queue, rendered bottom-left. New toasts go on top.
#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, toast: Toast) {
        // Cap the stack so a flood of errors cannot bury the UI.
        if self.items.len() >= 5 {
            self.items.remove(0);
        }
        self.items.push(toast);
    }

    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut [Toast] {
        &mut self.items
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
        }
    }

    /// Advance all toasts and drop finished ones.
    pub fn tick(&mut self, dt: f32) {
        self.items.retain_mut(|t| !t.tick(dt));
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_holds_then_slides_then_dies() {
        let mut t = Toast::info("hello");
        assert!(!t.tick(TOAST_HOLD_SECS - 0.1));
        assert_eq!(t.slide_offset(), 0.0); // still holding
        assert!(!t.tick(TOAST_SLIDE_SECS / 2.0)); // mid-slide
        assert!(t.slide_offset() > 0.0); // sliding
        assert!(t.tick(TOAST_SLIDE_SECS)); // fully slid away → gone
    }

    #[test]
    fn pinned_toast_never_ages() {
        let mut t = Toast::error("boom", Some("E-42".into()), None);
        t.pin();
        assert!(t.is_pinned());
        for _ in 0..100 {
            assert!(!t.tick(1.0));
        }
        assert_eq!(t.age(), 0.0);
        assert_eq!(t.slide_offset(), 0.0);
    }

    #[test]
    fn queue_is_capped() {
        let mut q = Toasts::default();
        for i in 0..10 {
            q.push(Toast::info(format!("t{i}")));
        }
        assert_eq!(q.items().len(), 5);
        assert_eq!(q.items().last().unwrap().title, "t9");
        assert_eq!(q.items()[0].title, "t5");
    }

    #[test]
    fn remove_by_index() {
        let mut q = Toasts::default();
        q.push(Toast::info("a"));
        q.push(Toast::info("b"));
        q.remove(0);
        assert_eq!(q.items().len(), 1);
        assert_eq!(q.items()[0].title, "b");
        q.remove(5); // out of range is a no-op
        assert_eq!(q.items().len(), 1);
    }
}
