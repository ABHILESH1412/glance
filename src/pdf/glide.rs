// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scrolling by key that eases to where it is going, rather than jumping
//! there in one frame.
//!
//! A key held down asks again before the last glide has landed. Each asks
//! from where the last one was heading, not from where the view happens to
//! be, so holding Down is one steady movement instead of a run of jerks, and
//! pressing Right three times quickly still goes three pages.

use std::cell::Cell;

use adw::prelude::*;

/// Long enough to follow, short enough that a single press feels immediate.
const DURATION_MS: u32 = 200;

pub struct Glide {
    adjustment: gtk::Adjustment,
    animation: adw::TimedAnimation,
    /// Where the glide under way will end.
    target: Cell<f64>,
}

impl Glide {
    /// Glides `adjustment`, timed by `widget`'s frame clock. With animations
    /// turned off in the desktop's settings, every glide is a jump.
    pub fn new(widget: &impl IsA<gtk::Widget>, adjustment: gtk::Adjustment) -> Self {
        let moved = adjustment.clone();
        let animation = adw::TimedAnimation::builder()
            .widget(widget)
            .target(&adw::CallbackAnimationTarget::new(move |value| moved.set_value(value)))
            .duration(DURATION_MS)
            .easing(adw::Easing::EaseOutCubic)
            .build();
        Self { adjustment, animation, target: Cell::new(0.0) }
    }

    fn moving(&self) -> bool {
        self.animation.state() == adw::AnimationState::Playing
    }

    /// Where the view is going: the end of the glide under way, or where it
    /// is when there is none.
    pub fn heading(&self) -> f64 {
        if self.moving() { self.target.get() } else { self.adjustment.value() }
    }

    /// The furthest the view can go.
    fn end(&self) -> f64 {
        let a = &self.adjustment;
        (a.upper() - a.page_size()).max(a.lower())
    }

    /// Glide on by `delta` from wherever the view is heading.
    pub fn by(&self, delta: f64) {
        self.to(self.heading() + delta);
    }

    /// Glide to `value`, kept within what can be scrolled to. Returns where
    /// the view will end up.
    pub fn to(&self, value: f64) -> f64 {
        let value = value.clamp(self.adjustment.lower(), self.end());
        self.target.set(value);
        // Played again, it starts over from where the view is now.
        self.animation.set_value_from(self.adjustment.value());
        self.animation.set_value_to(value);
        self.animation.play();
        value
    }

    /// Let go, where the view is: something else is moving it now.
    pub fn stop(&self) {
        if self.moving() {
            self.animation.pause();
        }
    }
}
