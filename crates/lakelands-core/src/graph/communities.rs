//! Communities: the Louvain method in integers, over a [`Held`] graph whose
//! steps were walked both ways.
//!
//! Every link is an edge, two relations between one pair are two, and a
//! link from a record to itself is a loop that counts twice in its
//! degree. Moving record `i`, of degree `k_i`, into community `C`, which
//! it has `k_{i,C}` edges into and whose degrees sum to `Σ_C` without it,
//! gains `2m·k_{i,C} − k_i·Σ_C`: the modularity gain times `2m²`, in
//! `i128`, with no division and no float. A record moves only where that
//! is strictly more than staying gains, so every move raises a bounded
//! integer and the sweeps end without a cap.
//!
//! Records are visited in `(type, key)` order, the store's one order, and
//! a tie goes to the community whose least member is least. A level is
//! swept until nothing moves, then each community becomes one record and
//! the next level begins; the first level that moves nothing ends it.
//! Louvain can leave a community in parts that no edge joins, so each one
//! is split into its connected parts, as `components` joins them. The
//! answer is a function of the graph and of its records' keys alone
//! (`docs/decisions/026-the-store-walks-the-graph.md`).

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::error::Case;
use lakelands_api::{Error, Result};

use super::Held;
use super::measures::{Component, grouped, join};

/// One level's graph: each record's edges to the others, and its degree,
/// its loops and the edges inside it counted from both ends. Records are
/// indexed by their least member, so index order is the order of visits
/// and of ties.
struct Level {
    edges: Vec<BTreeMap<u32, i128>>,
    degree: Vec<i128>,
}

/// The communities of `g`, ordered and numbered as components are: largest
/// first, then by their least `(type, key)`, each one's records by `(type,
/// key)`. `g`'s arcs are a link each way, as a step walked `both` holds
/// them. Refused `overflow` where a gain would pass `i128`.
pub fn communities(g: &Held) -> Result<Vec<Component>> {
    let of = louvain(g)?;
    // Each community split into its connected parts.
    let mut joined: Vec<usize> = (0..g.nodes.len()).collect();
    for &(u, v) in &g.arcs {
        if of[u as usize] == of[v as usize] {
            join(&mut joined, u as usize, v as usize);
        }
    }
    Ok(grouped(&g.nodes, &mut joined))
}

/// The community each of `g`'s records is in, as the levels leave it.
fn louvain(g: &Held) -> Result<Vec<u32>> {
    let n = g.nodes.len();
    let mut level = Level {
        edges: vec![BTreeMap::new(); n],
        degree: vec![0; n],
    };
    for &(u, v) in &g.arcs {
        // A loop is one arc, and counts twice.
        level.degree[u as usize] += if u == v { 2 } else { 1 };
        if u != v {
            *level.edges[u as usize].entry(v).or_default() += 1;
        }
    }
    let two_m: i128 = level.degree.iter().sum();
    // The record each of `g`'s records is at this level.
    let mut of: Vec<u32> = (0..n as u32).collect();
    loop {
        let (into, count) = sweep(&level, two_m)?;
        if count == level.degree.len() {
            return Ok(of);
        }
        for at in &mut of {
            *at = into[*at as usize];
        }
        level = aggregate(&level, &into, count);
    }
}

/// Sweeps one level until a sweep moves nothing: each record's community,
/// numbered by its least member, and how many there are.
fn sweep(level: &Level, two_m: i128) -> Result<(Vec<u32>, usize)> {
    let n = level.degree.len();
    let mut at: Vec<usize> = (0..n).collect();
    let mut sum = level.degree.clone();
    let mut members: Vec<BTreeSet<usize>> = (0..n).map(|i| BTreeSet::from([i])).collect();
    let gain = |k_ic: i128, k_i: i128, sum_c: i128| {
        let gain = (two_m.checked_mul(k_ic))
            .and_then(|a| k_i.checked_mul(sum_c).and_then(|b| a.checked_sub(b)));
        gain.ok_or(Error::invalid(Case::Overflow))
    };
    let mut moved = true;
    while moved {
        moved = false;
        for i in 0..n {
            let mut into: BTreeMap<usize, i128> = BTreeMap::new();
            for (&j, &w) in &level.edges[i] {
                *into.entry(at[j as usize]).or_default() += w;
            }
            let (here, k_i) = (at[i], level.degree[i]);
            sum[here] -= k_i;
            members[here].remove(&i);
            let stay = gain(into.get(&here).copied().unwrap_or(0), k_i, sum[here])?;
            let mut best = (stay, here);
            for (&c, &k_ic) in into.iter().filter(|(c, _)| **c != here) {
                let g = gain(k_ic, k_i, sum[c])?;
                let least = |c: usize| members[c].first().copied();
                if g > best.0 || (g == best.0 && best.1 != here && least(c) < least(best.1)) {
                    best = (g, c);
                }
            }
            let there = best.1;
            sum[there] += k_i;
            members[there].insert(i);
            if there != here {
                at[i] = there;
                moved = true;
            }
        }
    }
    // Numbered by least member: records are visited in that order, so the
    // first record met in a community is its least.
    let mut number: BTreeMap<usize, u32> = BTreeMap::new();
    let into = at
        .iter()
        .map(|c| {
            let next = number.len() as u32;
            *number.entry(*c).or_insert(next)
        })
        .collect();
    Ok((into, number.len()))
}

/// The next level: each community one record, its degree the sum of its
/// members' and its edges to each other community summed.
fn aggregate(level: &Level, into: &[u32], count: usize) -> Level {
    let mut next = Level {
        edges: vec![BTreeMap::new(); count],
        degree: vec![0; count],
    };
    for (i, edges) in level.edges.iter().enumerate() {
        let c = into[i] as usize;
        next.degree[c] += level.degree[i];
        for (&j, &w) in edges
            .iter()
            .filter(|(j, _)| into[**j as usize] as usize != c)
        {
            *next.edges[c].entry(into[j as usize]).or_default() += w;
        }
    }
    next
}

#[cfg(test)]
mod tests;
