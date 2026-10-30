// SPDX-License-Identifier: GPL-3.0-or-later
//! What both plugin formats need to play notes (roadmap 3.2): which
//! notes are sounding, so "all notes off" can stop each one, and how
//! an instrument's outputs reach the track and the tracks that take
//! them.

use daw_engine::midi::Outputs;

use crate::plugin::BusBufs;

/// The notes sounding on each channel, a bit per key.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Sounding([u128; 16]);

impl Sounding {
    pub(crate) fn on(&mut self, channel: u8, pitch: u8) {
        self.0[(channel & 15) as usize] |= 1 << (pitch & 127);
    }

    pub(crate) fn off(&mut self, channel: u8, pitch: u8) {
        self.0[(channel & 15) as usize] &= !(1 << (pitch & 127));
    }

    /// Each sounding (channel, pitch), forgetting them all.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = (u8, u8)> + use<> {
        let all = std::mem::take(&mut self.0);
        (0..16u8).flat_map(move |c| {
            (0..128u8)
                .filter(move |&p| all[c as usize] & (1 << p) != 0)
                .map(move |p| (c, p))
        })
    }
}

/// An instrument's output buses for `n` frames, at `at` into the block
/// `outs` covers: the main one added to `left` and `right`, each extra
/// one in `extra` to its own track when one takes it, else added too.
pub(crate) fn play_out(
    buses: &[BusBufs],
    extra: &[usize],
    left: &mut [f32],
    right: &mut [f32],
    outs: &mut Outputs<'_>,
    at: usize,
    n: usize,
) {
    let Some(main) = buses.first() else {
        return;
    };
    main.out(left, right, n, true);
    for &i in extra {
        let Some(bus) = buses.get(i) else { continue };
        match outs.get(i) {
            Some((l, r)) if l.len() >= at + n && r.len() >= at + n => {
                bus.out(&mut l[at..], &mut r[at..], n, false)
            }
            _ => bus.out(left, right, n, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounding_notes_come_back_once_each() {
        let mut s = Sounding::default();
        s.on(0, 60);
        s.on(9, 127);
        s.on(0, 64);
        s.off(0, 64);
        assert_eq!(s.drain().collect::<Vec<_>>(), vec![(0, 60), (9, 127)]);
        assert_eq!(s.drain().count(), 0);
    }
}
