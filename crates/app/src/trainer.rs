// SPDX-License-Identifier: GPL-3.0-or-later
//! The tempo trainer (roadmap 1.4): speed goes up by a step every few
//! times round the loop, until it reaches a target.

use eframe::egui;

pub struct Trainer {
    pub on: bool,
    /// Percentage points added each time.
    pub step: f64,
    /// Loops between steps.
    pub every: u64,
    /// Speed to stop at, percent.
    pub target: f64,
    /// Loop count when the last step was taken.
    since: Option<u64>,
}

impl Default for Trainer {
    fn default() -> Trainer {
        Trainer {
            on: false,
            step: 5.0,
            every: 2,
            target: 100.0,
            since: None,
        }
    }
}

impl Trainer {
    /// Given the engine's loop count, raises `speed` (percent) when it
    /// is time. Returns true if it changed.
    pub fn update(&mut self, loops: u64, speed: &mut f64) -> bool {
        if !self.on {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(loops);
        if loops < since + self.every.max(1) || *speed >= self.target {
            return false;
        }
        self.since = Some(loops);
        *speed = (*speed + self.step).min(self.target);
        true
    }

    /// The trainer's settings, on one row.
    pub fn controls(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.on, "Trainer")
            .on_hover_text("Speed up while looping: needs Loop on");
        ui.add_enabled_ui(self.on, |ui| {
            ui.add(
                egui::DragValue::new(&mut self.step)
                    .range(1.0..=25.0)
                    .speed(0.2)
                    .fixed_decimals(0)
                    .prefix("+")
                    .suffix(" %"),
            )
            .accessible_name("Trainer step, percent");
            ui.label("every");
            ui.add(
                egui::DragValue::new(&mut self.every)
                    .range(1..=32)
                    .suffix(" loops"),
            )
            .accessible_name("Trainer, loops between steps");
            ui.label("up to");
            ui.add(
                egui::DragValue::new(&mut self.target)
                    .range(50.0..=150.0)
                    .speed(0.5)
                    .fixed_decimals(0)
                    .suffix(" %"),
            )
            .accessible_name("Trainer target speed, percent");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_every_n_loops_up_to_the_target() {
        let mut t = Trainer {
            on: true,
            step: 10.0,
            every: 2,
            target: 85.0,
            since: None,
        };
        let mut speed = 60.0;
        let mut seen = Vec::new();
        for loops in 10..20 {
            if t.update(loops, &mut speed) {
                seen.push((loops, speed));
            }
        }
        assert_eq!(seen, vec![(12, 70.0), (14, 80.0), (16, 85.0)]);
    }

    #[test]
    fn off_does_nothing_and_starts_counting_afresh() {
        let mut t = Trainer::default();
        let mut speed = 70.0;
        assert!(!t.update(5, &mut speed));
        t.on = true;
        assert!(!t.update(9, &mut speed));
        assert!(!t.update(10, &mut speed));
        assert!(t.update(11, &mut speed));
        assert_eq!(speed, 75.0);
    }
}
