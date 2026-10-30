// SPDX-License-Identifier: GPL-3.0-or-later
//! A track's insert chain as the audio thread runs it, with effect
//! containers (roadmap 3.5): an effect that splits the audio into
//! several chains of effects (copies of it, frequency bands, mid and
//! side, or left and right) and joins them again.
//!
//! The session lists a track's effects flat, each naming the container
//! and chain it sits in, if any ([`InsertSpec::place`]). Compiling turns
//! that list into a tree of [`Step`]s, so effects keep their slots in
//! the flat list and are still taken over from graph to graph by id.
//!
//! Delay compensation inside a container: each chain waits as long as
//! its effects' latency, so every chain, and the untouched (dry) audio,
//! is delayed to match the slowest before they are joined. The
//! container's latency is that of its slowest chain.

use crate::graph::{Edge, InsertNode, InsertSpec, Lanes};
use crate::monitor::CueLine;

/// One step of a chain.
pub(crate) enum Step {
    /// The effect at this index of the track's inserts.
    Fx(usize),
    /// A container at this index, with its chains.
    Split(Box<SplitNode>),
}

pub(crate) struct SplitNode {
    /// The container's index in the track's inserts.
    at: usize,
    /// Each chain's audio.
    lanes: Lanes,
    chains: Vec<ChainNode>,
    /// The audio as it came in, delayed by the slowest chain.
    dry: [Vec<f32>; 2],
    dry_line: CueLine,
}

struct ChainNode {
    steps: Vec<Step>,
    /// Lines the chain up with the slowest one.
    line: CueLine,
}

/// What effects listening to a sidechain need from the graph.
pub(crate) struct Keys<'a> {
    pub edges: &'a mut [Edge],
    pub post: &'a [[Vec<f32>; 2]],
    pub key: &'a mut [Vec<f32>; 2],
}

/// The top of a track's chain, built from its flat list of inserts.
pub(crate) struct Built {
    pub steps: Vec<Step>,
    /// The chain's latency: of the effects not bypassed, and of the
    /// slowest chain of each container.
    pub latency: i64,
    /// For each insert, how long after the track's input it runs: the
    /// latency of what comes before it. Lines up its sidechain.
    pub before: Vec<i64>,
    /// Each layer's chain, for a layered instrument (see
    /// [`crate::layers`]), in the order the layers were given.
    pub layers: Vec<LayerChain>,
}

/// The effects of one layer of a layered instrument.
pub(crate) struct LayerChain {
    pub steps: Vec<Step>,
    /// Lines the layer up with the slowest one.
    pub line: CueLine,
}

/// Builds the tree from a track's inserts. An effect naming a container
/// that isn't on the track sits at the top of the chain; one in a chain
/// past the container's last, or in a container that is itself never
/// reached, doesn't run.
///
/// `layers` are a layered instrument's layers, by id with their
/// instrument's latency: the effects naming a layer as their container
/// make its chain. The layers run before the track's chain, so it waits
/// for the slowest of them.
pub(crate) fn build(specs: &[InsertSpec], layers: &[(u64, i64)], max_block: usize) -> Built {
    let mut before = vec![0; specs.len()];
    let top = |s: &InsertSpec| match s.place {
        None => true,
        Some(p) => {
            !layers.iter().any(|l| l.0 == p.container)
                && !specs.iter().any(|c| c.id == p.container && c.branches > 0)
        }
    };
    let mut own: Vec<(Vec<Step>, i64)> = (layers.iter())
        .map(|&(id, latency)| {
            let inside: Vec<usize> = (0..specs.len())
                .filter(|&j| specs[j].place.is_some_and(|p| p.container == id))
                .collect();
            let (steps, l) = chain(specs, &inside, latency, max_block, &mut before);
            (steps, latency + l)
        })
        .collect();
    let slowest = own.iter().map(|c| c.1).max().unwrap_or(0);
    let layers = own
        .drain(..)
        .map(|(steps, l)| LayerChain {
            steps,
            line: CueLine::new((slowest - l) as usize),
        })
        .collect();
    let roots: Vec<usize> = (0..specs.len()).filter(|&k| top(&specs[k])).collect();
    let (steps, latency) = chain(specs, &roots, slowest, max_block, &mut before);
    Built {
        steps,
        latency: slowest + latency,
        before,
        layers,
    }
}

/// The steps for the inserts at `ks`, in order, starting `start` frames
/// after the track's input. Returns them with their latency.
fn chain(
    specs: &[InsertSpec],
    ks: &[usize],
    start: i64,
    max_block: usize,
    before: &mut [i64],
) -> (Vec<Step>, i64) {
    let mut steps = Vec::with_capacity(ks.len());
    let mut at = start;
    for &k in ks {
        let s = &specs[k];
        before[k] = at;
        if s.branches == 0 {
            steps.push(Step::Fx(k));
            if !s.bypass {
                at += s.latency as i64;
            }
            continue;
        }
        let count = (s.branches as usize).min(crate::graph::MAX_BRANCHES);
        let mut built: Vec<(Vec<Step>, i64)> = (0..count)
            .map(|b| {
                let inside: Vec<usize> = (0..specs.len())
                    .filter(|&j| {
                        specs[j]
                            .place
                            .is_some_and(|p| p.container == s.id && p.branch as usize == b)
                    })
                    .collect();
                chain(specs, &inside, at, max_block, before)
            })
            .collect();
        let slowest = built.iter().map(|c| c.1).max().unwrap_or(0);
        let chains = built
            .drain(..)
            .map(|(steps, l)| ChainNode {
                steps,
                line: CueLine::new((slowest - l) as usize),
            })
            .collect();
        steps.push(Step::Split(Box::new(SplitNode {
            at: k,
            lanes: Lanes::new(count, max_block),
            chains,
            dry: [vec![0.0; max_block], vec![0.0; max_block]],
            dry_line: CueLine::new(slowest as usize),
        })));
        if !s.bypass {
            at += slowest;
        }
    }
    (steps, at - start)
}

/// Runs a chain over one block in place.
pub(crate) fn run(
    steps: &mut [Step],
    inserts: &mut [InsertNode],
    keys: &mut Keys,
    l: &mut [f32],
    r: &mut [f32],
) {
    let n = l.len();
    for step in steps {
        match step {
            Step::Fx(k) => run_fx(&mut inserts[*k], keys, l, r),
            Step::Split(sp) => {
                let sp = &mut **sp;
                if inserts[sp.at].bypass {
                    continue;
                }
                let [dl, dr] = &mut sp.dry;
                let (dl, dr) = (&mut dl[..n], &mut dr[..n]);
                dl.copy_from_slice(l);
                dr.copy_from_slice(r);
                sp.lanes.set_frames(n);
                match inserts[sp.at].effect.as_mut().and_then(|e| e.as_split()) {
                    Some(split) => split.split(l, r, &mut sp.lanes),
                    // Not handed in: the chains hear nothing, and the
                    // dry audio passes.
                    None => sp.lanes.clear(),
                }
                for (b, c) in sp.chains.iter_mut().enumerate() {
                    let (bl, br) = sp.lanes.get_mut(b);
                    run(&mut c.steps, inserts, keys, bl, br);
                    c.line.run(bl, br);
                }
                sp.dry_line.run(dl, dr);
                match inserts[sp.at].effect.as_mut().and_then(|e| e.as_split()) {
                    Some(split) => split.join((dl, dr), &sp.lanes, l, r),
                    None => {
                        l.copy_from_slice(dl);
                        r.copy_from_slice(dr);
                    }
                }
            }
        }
    }
}

fn run_fx(i: &mut InsertNode, keys: &mut Keys, l: &mut [f32], r: &mut [f32]) {
    let (false, Some(fx)) = (i.bypass, &mut i.effect) else {
        return;
    };
    let n = l.len();
    match i.key {
        Some(e) => {
            let e = &mut keys.edges[e];
            let [kl, kr] = &mut *keys.key;
            let (kl, kr) = (&mut kl[..n], &mut kr[..n]);
            kl.fill(0.0);
            kr.fill(0.0);
            let from = &keys.post[e.from];
            e.feed((&from[0][..n], &from[1][..n]), (&mut *kl, &mut *kr));
            fx.process_keyed(l, r, (kl, kr));
        }
        None => fx.process(l, r),
    }
}
