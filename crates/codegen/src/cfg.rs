//! Each function's control-flow graph and its shape, for structured
//! codegen (`docs/structured-codegen-design.md`, step 3): reverse
//! postorder, dominators, the loop nest, reducibility and the irreducible
//! regions a structurer keeps as `loop { match pc }`.
//!
//! The CFG is codegen's own: a node per block of the [`Func`], an edge per
//! successor inside it ([`func::successors`]), so a call is an edge to its
//! return site. Node 0 is the entry. Blocks no edge reaches from the entry
//! are left out and listed ([`Cfg::unreachable`]): they can only be entered
//! through the dispatcher. That happens when a block that was built whole
//! is later split by a new start inside it, and the first build folded a
//! value the second can't see. The case found (Anim_FourPetal in
//! pokeemerald_modern): `movs r3, #0` then a jump table discovery didn't
//! resolve (its bound is checked on `r3 & 0xffff`, the index is `r3`), so
//! the first build folded the table load to case 0's block and built it; a
//! back edge to the instruction after the `movs` then split the block, and
//! the rebuilt jump is an unresolved one with no successors.

use crate::func::{self, Ctx, Func};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// One function's CFG: node 0 is the entry, every node is reachable from
/// it.
#[derive(Clone, Debug)]
pub struct Cfg {
    /// Each node's block key.
    pub keys: Vec<u32>,
    pub succ: Vec<Vec<usize>>,
    pub pred: Vec<Vec<usize>>,
    /// Blocks of the function these edges don't reach from its entry.
    pub unreachable: Vec<u32>,
}

impl Cfg {
    /// The CFG of `f`'s blocks.
    pub fn of(ctx: &Ctx, f: &Func) -> Cfg {
        let mut keys: Vec<u32> = vec![f.entry];
        keys.extend(f.blocks.keys().copied().filter(|&k| k != f.entry));
        let index: BTreeMap<u32, usize> = keys.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let succ = keys
            .iter()
            .map(|k| {
                let out: BTreeSet<usize> = func::exits(ctx, &f.blocks[k])
                    .iter()
                    .flat_map(|x| func::successors(ctx, x))
                    .filter_map(|s| index.get(&s).copied())
                    .collect();
                out.into_iter().collect()
            })
            .collect();
        Cfg::new(keys, succ)
    }

    /// The CFG with these nodes and edges (node 0 the entry), less the
    /// nodes it doesn't reach from the entry. Edges keep their order.
    pub fn new(keys: Vec<u32>, succ: Vec<Vec<usize>>) -> Cfg {
        let n = keys.len();
        let mut seen = vec![false; n];
        let mut work = vec![0];
        seen[0] = true;
        while let Some(v) = work.pop() {
            for &w in &succ[v] {
                if !seen[w] {
                    seen[w] = true;
                    work.push(w);
                }
            }
        }
        let mut new = vec![usize::MAX; n];
        let mut m = 0;
        for v in 0..n {
            if seen[v] {
                new[v] = m;
                m += 1;
            }
        }
        let mut pred = vec![Vec::new(); m];
        let mut kept = Vec::with_capacity(m);
        for v in (0..n).filter(|&v| seen[v]) {
            let out: Vec<usize> = succ[v].iter().map(|&w| new[w]).collect();
            for &w in &out {
                pred[w].push(new[v]);
            }
            kept.push(out);
        }
        Cfg {
            unreachable: (0..n).filter(|&v| !seen[v]).map(|v| keys[v]).collect(),
            keys: (0..n).filter(|&v| seen[v]).map(|v| keys[v]).collect(),
            succ: kept,
            pred,
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The nodes in reverse postorder of a depth-first walk from the entry
    /// that takes each node's edges in order.
    pub fn reverse_postorder(&self) -> Vec<usize> {
        let n = self.len();
        let mut seen = vec![false; n];
        let mut post = Vec::with_capacity(n);
        let mut stack = vec![(0usize, 0usize)];
        seen[0] = true;
        while let Some(&mut (v, ref mut i)) = stack.last_mut() {
            if let Some(&w) = self.succ[v].get(*i) {
                *i += 1;
                if !seen[w] {
                    seen[w] = true;
                    stack.push((w, 0));
                }
            } else {
                post.push(v);
                stack.pop();
            }
        }
        post.reverse();
        post
    }
}

/// A loop or irreducible region: a strongly connected region of the CFG
/// once the edges into enclosing regions' headers are cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// The region it is nested in, by index in [`Shape::regions`].
    pub parent: Option<usize>,
    /// 0 for an outermost region.
    pub depth: usize,
    /// Its nodes, ascending.
    pub blocks: Vec<usize>,
    /// Its nodes with a predecessor outside it (or the entry), ascending:
    /// one, the header, for a natural loop.
    pub entries: Vec<usize>,
    /// The distinct nodes outside it that its edges lead to, ascending.
    pub exits: Vec<usize>,
}

impl Region {
    /// A natural loop: entered only at its header.
    pub fn is_loop(&self) -> bool {
        self.entries.len() == 1
    }
}

/// What a structurer needs to know about a CFG.
#[derive(Clone, Debug)]
pub struct Shape {
    pub rpo: Vec<usize>,
    /// Each node's place in [`Shape::rpo`].
    pub rpo_ix: Vec<usize>,
    /// Each node's immediate dominator; the entry's is itself.
    pub idom: Vec<usize>,
    /// Every retreating edge (in reverse postorder) goes to a node that
    /// dominates its source.
    pub reducible: bool,
    /// The loop nest, Steensgaard-style: outer regions before inner,
    /// depth-first.
    pub regions: Vec<Region>,
    /// How deep Ramsey's translation nests a block (labeled blocks and
    /// loops), at most; 0 for an irreducible function.
    pub nesting: usize,
}

impl Shape {
    pub fn of(g: &Cfg) -> Shape {
        let n = g.len();
        let rpo = g.reverse_postorder();
        let mut rpo_ix = vec![0; n];
        for (i, &v) in rpo.iter().enumerate() {
            rpo_ix[v] = i;
        }
        let idom = idoms(g, &rpo, &rpo_ix);
        let mut reducible = true;
        let mut header = vec![false; n];
        let mut forward_in = vec![0usize; n];
        for u in 0..n {
            for &v in &g.succ[u] {
                if rpo_ix[v] <= rpo_ix[u] {
                    header[v] = true;
                    reducible &= dominates(&idom, v, u);
                } else {
                    forward_in[v] += 1;
                }
            }
        }
        let mut regions = Vec::new();
        let all: Vec<usize> = (0..n).collect();
        nest(g, &all, &mut HashSet::new(), None, 0, &mut regions);
        // Ramsey: a node's code sits inside a block for each of its
        // dominator tree children that is a merge node (two or more forward
        // edges in), and a loop header's code inside its `loop`.
        let mut nesting = 0;
        if reducible {
            let mut merge_children = vec![0usize; n];
            for v in 1..n {
                if forward_in[v] >= 2 {
                    merge_children[idom[v]] += 1;
                }
            }
            let mut d = vec![0usize; n];
            for &v in &rpo {
                d[v] = if v == 0 {
                    0
                } else {
                    d[idom[v]] + merge_children[idom[v]]
                } + header[v] as usize;
                nesting = nesting.max(d[v] + merge_children[v]);
            }
        }
        Shape {
            rpo,
            rpo_ix,
            idom,
            reducible,
            regions,
            nesting,
        }
    }

    /// Whether `a` dominates `b`.
    pub fn dominates(&self, a: usize, b: usize) -> bool {
        dominates(&self.idom, a, b)
    }

    /// The outermost irreducible regions: each is collapsed into one node,
    /// kept as `loop { match pc }` over its blocks, with the regions nested
    /// in it.
    pub fn irreducible(&self) -> Vec<&Region> {
        let mut out = Vec::new();
        let mut inside_until: Option<usize> = None;
        for r in &self.regions {
            if let Some(d) = inside_until {
                if r.depth > d {
                    continue;
                }
                inside_until = None;
            }
            if !r.is_loop() {
                out.push(r);
                inside_until = Some(r.depth);
            }
        }
        out
    }
}

/// Immediate dominators (Cooper, Harvey and Kennedy), by node; `rpo_ix`
/// is each node's place in reverse postorder.
fn idoms(g: &Cfg, rpo: &[usize], rpo_ix: &[usize]) -> Vec<usize> {
    let n = g.len();
    let undef = usize::MAX;
    let mut idom = vec![undef; n];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &rpo[1..] {
            let mut new = undef;
            for &p in &g.pred[b] {
                if idom[p] == undef {
                    continue;
                }
                new = if new == undef {
                    p
                } else {
                    let (mut x, mut y) = (p, new);
                    while x != y {
                        while rpo_ix[x] > rpo_ix[y] {
                            x = idom[x];
                        }
                        while rpo_ix[y] > rpo_ix[x] {
                            y = idom[y];
                        }
                    }
                    x
                };
            }
            if idom[b] != new {
                idom[b] = new;
                changed = true;
            }
        }
    }
    idom
}

fn dominates(idom: &[usize], a: usize, mut b: usize) -> bool {
    loop {
        if a == b {
            return true;
        }
        if b == 0 {
            return false;
        }
        b = idom[b];
    }
}

/// Tarjan's strongly connected components of the nodes in `set`, without
/// the edges in `cut`, in the order Tarjan finishes them. Iterative: a
/// region may have thousands of blocks.
fn sccs(g: &Cfg, set: &[usize], cut: &HashSet<(usize, usize)>) -> Vec<Vec<usize>> {
    const NONE: usize = usize::MAX;
    let n = g.len();
    let mut inside = vec![false; n];
    for &v in set {
        inside[v] = true;
    }
    // Index and lowlink by node, NONE before it's visited.
    let mut index = vec![(NONE, NONE); n];
    let mut on = vec![false; n];
    let mut stack = Vec::new();
    let mut next = 0;
    let mut out = Vec::new();
    for &root in set {
        if index[root].0 != NONE {
            continue;
        }
        // (node, next edge to look at, the child just returned from)
        let mut calls = vec![(root, 0usize, NONE)];
        index[root] = (next, next);
        next += 1;
        stack.push(root);
        on[root] = true;
        while let Some(&mut (v, ref mut i, ref mut child)) = calls.last_mut() {
            if *child != NONE {
                index[v].1 = index[v].1.min(index[*child].1);
                *child = NONE;
            }
            if let Some(&w) = g.succ[v].get(*i) {
                *i += 1;
                if !inside[w] || cut.contains(&(v, w)) {
                    continue;
                }
                if index[w].0 == NONE {
                    calls.last_mut().unwrap().2 = w;
                    index[w] = (next, next);
                    next += 1;
                    stack.push(w);
                    on[w] = true;
                    calls.push((w, 0, NONE));
                } else if on[w] {
                    index[v].1 = index[v].1.min(index[w].0);
                }
                continue;
            }
            let (i, low) = index[v];
            if i == low {
                let mut c = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on[w] = false;
                    c.push(w);
                    if w == v {
                        break;
                    }
                }
                out.push(c);
            }
            calls.pop();
        }
    }
    out
}

/// The loop nest, Steensgaard-style: every cyclic SCC is a region, its
/// entries are its headers, and cutting the edges into them from inside
/// exposes the regions nested in it.
fn nest(
    g: &Cfg,
    set: &[usize],
    cut: &mut HashSet<(usize, usize)>,
    parent: Option<usize>,
    depth: usize,
    out: &mut Vec<Region>,
) {
    for c in sccs(g, set, cut) {
        let inside: HashSet<usize> = c.iter().copied().collect();
        let cyclic = c.len() > 1
            || g.succ[c[0]]
                .iter()
                .any(|&w| w == c[0] && !cut.contains(&(w, w)));
        if !cyclic {
            continue;
        }
        let entries: Vec<usize> = c
            .iter()
            .copied()
            .filter(|&v| v == 0 || g.pred[v].iter().any(|p| !inside.contains(p)))
            .collect();
        let exits: BTreeSet<usize> = c
            .iter()
            .flat_map(|&v| g.succ[v].iter().copied())
            .filter(|w| !inside.contains(w))
            .collect();
        for &h in &entries {
            for &p in &g.pred[h] {
                if inside.contains(&p) {
                    cut.insert((p, h));
                }
            }
        }
        let mut blocks = c.clone();
        blocks.sort_unstable();
        let mut entries = entries;
        entries.sort_unstable();
        let me = out.len();
        out.push(Region {
            parent,
            depth,
            blocks,
            entries,
            exits: exits.into_iter().collect(),
        });
        nest(g, &c, cut, Some(me), depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CFG of `n` nodes keyed by their index, with these edges.
    fn graph(n: usize, edges: &[(usize, usize)]) -> Cfg {
        let mut succ = vec![Vec::new(); n];
        for &(a, b) in edges {
            succ[a].push(b);
        }
        Cfg::new((0..n as u32).collect(), succ)
    }

    fn region(
        parent: Option<usize>,
        depth: usize,
        blocks: &[usize],
        entries: &[usize],
        exits: &[usize],
    ) -> Region {
        Region {
            parent,
            depth,
            blocks: blocks.to_vec(),
            entries: entries.to_vec(),
            exits: exits.to_vec(),
        }
    }

    #[test]
    fn diamond() {
        let s = Shape::of(&graph(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]));
        assert!(s.reducible);
        assert_eq!(s.idom, [0, 0, 0, 0]);
        assert!(s.regions.is_empty());
        assert_eq!(s.rpo[0], 0);
        assert_eq!(s.rpo[3], 3);
        // 3 is a merge node: one labeled block around 1 and 2.
        assert_eq!(s.nesting, 1);
    }

    #[test]
    fn nested_loops() {
        // 1 heads a loop with 2, which loops on itself; 3 leaves.
        let g = graph(4, &[(0, 1), (1, 2), (2, 2), (2, 1), (1, 3)]);
        let s = Shape::of(&g);
        assert!(s.reducible);
        assert_eq!(s.idom, [0, 0, 1, 1]);
        assert!(s.dominates(1, 2) && !s.dominates(2, 3));
        assert_eq!(
            s.regions,
            [
                region(None, 0, &[1, 2], &[1], &[3]),
                region(Some(0), 1, &[2], &[2], &[1]),
            ]
        );
        assert!(s.irreducible().is_empty());
        // 2 inside both loops.
        assert_eq!(s.nesting, 2);
    }

    #[test]
    fn entry_heads_a_loop() {
        let s = Shape::of(&graph(3, &[(0, 1), (1, 0), (1, 2)]));
        assert!(s.reducible);
        assert_eq!(s.regions, [region(None, 0, &[0, 1], &[0], &[2])]);
    }

    #[test]
    fn several_exits() {
        let s = Shape::of(&graph(5, &[(0, 1), (1, 2), (1, 3), (2, 1), (2, 4), (3, 4)]));
        assert!(s.reducible);
        assert_eq!(s.regions, [region(None, 0, &[1, 2], &[1], &[3, 4])]);
    }

    #[test]
    fn irreducible() {
        // 1 and 2 jump to each other, and the entry reaches both.
        let s = Shape::of(&graph(4, &[(0, 1), (0, 2), (1, 2), (2, 1), (2, 3)]));
        assert!(!s.reducible);
        assert_eq!(s.regions, [region(None, 0, &[1, 2], &[1, 2], &[3])]);
        assert_eq!(s.irreducible(), [&s.regions[0]]);
        assert_eq!(s.nesting, 0);
    }

    #[test]
    fn loop_inside_an_irreducible_region() {
        // {1, 2, 3} is entered at 1 and 2; with the edges into them cut, 3
        // still loops on itself. A loop beside it stays outside.
        let g = graph(
            7,
            &[
                (0, 1),
                (0, 2),
                (1, 2),
                (2, 3),
                (3, 3),
                (3, 1),
                (3, 4),
                (4, 5),
                (5, 4),
                (5, 6),
            ],
        );
        let s = Shape::of(&g);
        assert!(!s.reducible);
        let outer = region(None, 0, &[1, 2, 3], &[1, 2], &[4]);
        assert!(s.regions.contains(&outer));
        assert!(s.regions.contains(&region(
            Some(s.regions.iter().position(|r| *r == outer).unwrap()),
            1,
            &[3],
            &[3],
            &[1, 4]
        )));
        assert!(s.regions.contains(&region(None, 0, &[4, 5], &[4], &[6])));
        assert_eq!(s.irreducible(), [&outer]);
    }

    #[test]
    fn irreducible_inside_a_loop() {
        // A loop at 1 whose body 2/3 is entered at both.
        let g = graph(5, &[(0, 1), (1, 2), (1, 3), (2, 3), (3, 2), (3, 1), (1, 4)]);
        let s = Shape::of(&g);
        assert!(!s.reducible);
        assert_eq!(
            s.regions,
            [
                region(None, 0, &[1, 2, 3], &[1], &[4]),
                region(Some(0), 1, &[2, 3], &[2, 3], &[1]),
            ]
        );
        assert_eq!(s.irreducible(), [&s.regions[1]]);
    }

    #[test]
    fn unreachable_blocks_are_left_out() {
        let mut succ = vec![vec![2], vec![2], vec![]];
        succ[1].push(0);
        let g = Cfg::new(vec![0x10, 0x20, 0x30], succ);
        assert_eq!(g.keys, [0x10, 0x30]);
        assert_eq!(g.unreachable, [0x20]);
        assert_eq!(g.succ, [vec![1], vec![]]);
        assert_eq!(g.pred, [vec![], vec![0]]);
    }

    #[test]
    fn a_long_cycle_does_not_recurse() {
        let n = 200_000;
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|v| (v, v + 1)).collect();
        edges.push((n - 1, 1));
        let s = Shape::of(&graph(n, &edges));
        assert!(s.reducible);
        assert_eq!(s.regions.len(), 1);
        assert_eq!(s.regions[0].blocks.len(), n - 1);
        assert_eq!(s.regions[0].entries, [1]);
    }
}
