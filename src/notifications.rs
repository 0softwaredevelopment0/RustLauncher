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

/// Seconds a fresh toast takes to fade in.
pub const TOAST_APPEAR_SECS: f32 = 0.25;

/// Expand/collapse animation speed for the log section (progress per second).
const TOAST_EXPAND_SPEED: f32 = 5.0;

/// The expanded log section's maximum height in pixels.
pub const TOAST_MAX_LOG_HEIGHT: f32 = 180.0;

/// Cap for the full log attached to an error toast (last N lines).
pub const TOAST_MAX_LOG_LINES: usize = 300;

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
    #[allow(dead_code)] // kept for API completeness; the header shows it inline
    pub code: Option<String>,
    /// Optional multi-line detail (for errors: the last log lines).
    pub detail: Option<String>,
    /// Seconds since the toast was created.
    age: f32,
    /// Pinned by a left-click: stops aging until dismissed.
    pinned: bool,
    /// Height measured during the last render; used to stack toasts.
    height: f32,
    /// The full (capped) log for error toasts, revealed by expansion.
    full_log: Option<String>,
    /// Whether the log section is currently expanded.
    expanded: bool,
    /// 0..1 animation progress of the expansion.
    expand_progress: f32,
}

impl Toast {
    pub fn error(
        title: impl Into<String>,
        code: Option<String>,
        detail: Option<String>,
        full_log: Option<String>,
    ) -> Self {
        Toast {
            kind: ToastKind::Error,
            title: title.into(),
            code,
            detail,
            age: 0.0,
            pinned: false,
            height: 0.0,
            full_log,
            expanded: false,
            expand_progress: 0.0,
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
            height: 0.0,
            full_log: None,
            expanded: false,
            expand_progress: 0.0,
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

    /// Left-click on an error toast: expand the full log smoothly upward.
    /// Clicking again collapses it back and resumes the normal 5s hold.
    pub fn toggle_expanded(&mut self) {
        if self.full_log.is_none() {
            return;
        }
        if self.expanded {
            // Collapse: hide the log, release the pin and restart the hold.
            self.expanded = false;
            self.pinned = false;
            self.age = 0.0;
        } else {
            self.expanded = true;
            self.pinned = true;
        }
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    pub fn expand_progress(&self) -> f32 {
        self.expand_progress
    }

    pub fn full_log(&self) -> Option<&str> {
        self.full_log.as_deref()
    }

    /// Advance the age and the expansion animation; returns `true` when an
    /// unpinned toast has fully slid away and should be removed.
    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.pinned {
            self.age += dt;
        }
        // The expansion animates regardless of the pin state.
        let target = if self.expanded { 1.0 } else { 0.0 };
        let speed = TOAST_EXPAND_SPEED * dt;
        if self.expand_progress < target {
            self.expand_progress = (self.expand_progress + speed).min(target);
        } else if self.expand_progress > target {
            self.expand_progress = (self.expand_progress - speed).max(target);
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

    /// Opacity for rendering: fades in on spawn and out while sliding away.
    pub fn visual_alpha(&self) -> f32 {
        if self.pinned {
            return 1.0;
        }
        if self.age < TOAST_APPEAR_SECS {
            (self.age / TOAST_APPEAR_SECS).clamp(0.0, 1.0)
        } else if self.age > TOAST_HOLD_SECS {
            (1.0 - (self.age - TOAST_HOLD_SECS) / TOAST_SLIDE_SECS).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Progress fraction of the countdown bar: 1.0 right after appearing,
    /// draining to 0.0 over the hold time, then gone. Pinned toasts report
    /// 0.0 so the bar disappears once a click locks the toast.
    pub fn hold_frac(&self) -> f32 {
        if self.pinned {
            return 0.0;
        }
        ((TOAST_HOLD_SECS - self.age) / TOAST_HOLD_SECS).clamp(0.0, 1.0)
    }

    /// The height measured during the last render (0 = not rendered yet).
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn height(&self) -> f32 {
        self.height
    }

    pub fn set_height(&mut self, height: f32) {
        if height > 0.0 {
            self.height = height;
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
        let mut t = Toast::error("boom", Some("E-42".into()), None, Some("log".into()));
        t.pin();
        assert!(t.is_pinned());
        for _ in 0..100 {
            assert!(!t.tick(1.0));
        }
        assert_eq!(t.age(), 0.0);
        assert_eq!(t.slide_offset(), 0.0);
    }

    #[test]
    fn expand_toggles_and_animates() {
        let mut t = Toast::error("boom", None, None, Some("line1\nline2".into()));
        assert!(!t.expanded());
        t.toggle_expanded();
        assert!(t.expanded() && t.is_pinned());
        t.tick(0.1);
        assert!(t.expand_progress() > 0.0 && t.expand_progress() < 1.0);
        for _ in 0..60 {
            t.tick(0.05);
        }
        assert_eq!(t.expand_progress(), 1.0);
        assert!(!t.tick(10.0)); // pinned while expanded → never slides
        t.toggle_expanded(); // collapse
        assert!(!t.expanded() && !t.is_pinned());
        for _ in 0..60 {
            t.tick(0.05);
        }
        assert_eq!(t.expand_progress(), 0.0);
        assert!(t.tick(TOAST_HOLD_SECS + TOAST_SLIDE_SECS)); // resumes aging
    }

    #[test]
    fn toggle_is_a_no_op_without_a_log() {
        let mut t = Toast::error("boom", None, None, None);
        t.toggle_expanded();
        assert!(!t.expanded());
    }

    #[test]
    fn visual_alpha_fades_in_and_out() {
        let mut t = Toast::info("x");
        t.tick(0.05); // mid fade-in
        assert!(t.visual_alpha() > 0.0 && t.visual_alpha() < 1.0);
        t.tick(1.0); // fully visible
        assert_eq!(t.visual_alpha(), 1.0);
        let mut t2 = Toast::info("x");
        t2.tick(TOAST_HOLD_SECS + TOAST_SLIDE_SECS / 2.0);
        assert!(t2.visual_alpha() > 0.0 && t2.visual_alpha() < 1.0);
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
