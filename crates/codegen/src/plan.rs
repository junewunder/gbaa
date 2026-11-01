//! The structuring plan of a function and its structure check
//! (`docs/structured-codegen-design.md`, step 4): Ramsey's "Beyond
//! Relooper" over the dominator tree, with each irreducible region
//! collapsed into one node kept as `loop { match pc }`.
//!
//! The plan is a tree over the function's [`Cfg`] nodes. It places every
//! block exactly once and gives every edge (an exit of a block and one of
//! its targets) exactly one counterpart, so the printer copies no code and
//! evaluates no condition twice: each edge keeps its own flag writes,
//! cycles, tick and yield `return`, and only what follows the tick changes
//! (`pc = K; continue` becomes a `break`, a `continue`, a fall-through or
//! the target's code inline). The design's `If` is a conditional exit whose
//! edge is [`How::Inline`], its `Match` an exit with several targets or a
//! fallback.
//!
//! [`check`] is the structure check every plan passes before it is used
//! (CompCert's Linearize pattern): every block once, every edge to its own
//! target with its exit's fallback kept, every label in scope. What the
//! plan doesn't hold (each block's IR, `insn!` markers, guards and
//! versions) is the printer's to keep, and its check's.

use crate::cfg::{Cfg, Shape};
use crate::func::{self, Ctx, Func};
use std::collections::{BTreeMap, BTreeSet};

/// One exit of a block, as the planner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    /// The nodes it may continue at, distinct, in order.
    pub targets: Vec<usize>,
    /// It may also leave the function: a computed jump's or a jump
    /// table's `_` arm to the dispatcher, a call's guard on its return, a
    /// jump to another function, a return.
    pub leaves: bool,
}

/// Each node's exits in `g`, from the function's blocks: what
/// [`func::successors`] gives, an exit that isn't a plain static jump
/// inside the function leaving as well.
pub fn exits_of(ctx: &Ctx, f: &Func, g: &Cfg) -> Vec<Vec<Exit>> {
    let index: BTreeMap<u32, usize> = g.keys.iter().enumerate().map(|(i, &k)| (k, i)).collect();
    g.keys
        .iter()
        .map(|k| {
            func::exits(ctx, &f.blocks[k])
                .iter()
                .map(|x| {
                    let mut targets = Vec::new();
                    let mut leaves = !matches!(x.plan, func::Plan::Static(_));
                    for s in func::successors(ctx, x) {
                        match index.get(&s) {
                            Some(t) if !targets.contains(t) => targets.push(*t),
                            Some(_) => {}
                            None => leaves = true,
                        }
                    }
                    Exit { targets, leaves }
                })
                .collect()
        })
        .collect()
}

/// What an edge does after its tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum How {
    /// To the code after the enclosing [`Node::Labeled`] with this label.
    Break(usize),
    /// To the start of the enclosing [`Node::Loop`] with this label.
    Continue(usize),
    /// Falls off the end of what it's in into its target: a break to the
    /// labeled block it ends.
    Next,
    /// Its target's code follows here, reached by this edge alone.
    Inline(Vec<Node>),
    /// Inside a [`Node::Region`], to another of its blocks.
    Goto,
    /// Inside a [`Node::Region`], out of it, to its arm in the region's
    /// exits.
    Leave,
}

/// One edge: its target node and what it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub to: usize,
    pub how: How,
}

/// One exit of a block, planned: an edge per target in the exit's order,
/// and whether it keeps its `return` to the dispatcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Out {
    pub edges: Vec<Edge>,
    pub fallback: bool,
}

/// A node of the plan. Labels are node numbers: a loop's its header's, a
/// labeled block's the merge node after it, a region's its lowest entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// A block's code, its exits in order, the last unconditional.
    Block {
        node: usize,
        exits: Vec<Out>,
    },
    Loop {
        label: usize,
        body: Vec<Node>,
    },
    /// The block Ramsey puts around a merge node's predecessors; the merge
    /// node's code follows it.
    Labeled {
        label: usize,
        body: Vec<Node>,
    },
    /// A collapsed irreducible region, printed as v1's `loop { match pc }`
    /// over its blocks (in node order) from the entry its incoming edge
    /// takes; leaving it, `match pc` over `exits` picks the edge on.
    Region {
        label: usize,
        entries: Vec<usize>,
        blocks: Vec<Node>,
        exits: Vec<Edge>,
    },
}

/// A function's plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub tree: Vec<Node>,
    /// Its collapsed regions, and the blocks in them.
    pub regions: usize,
    pub region_blocks: usize,
}

/// Why a function has no plan (and stays v1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// Still irreducible once its irreducible regions are collapsed.
    Irreducible,
    /// The plan failed the structure check.
    Check(String),
}

/// The plan of `g` (whose nodes have these exits), checked.
pub fn plan(g: &Cfg, exits: &[Vec<Exit>]) -> Result<Planned, Fail> {
    let p = plan_unchecked(g, exits)?;
    check(g, exits, &p.tree).map_err(Fail::Check)?;
    Ok(p)
}

/// The plan of a function's blocks: its CFG and its plan.
pub fn of(ctx: &Ctx, f: &Func) -> (Cfg, Result<Planned, Fail>) {
    let g = Cfg::of(ctx, f);
    let x = exits_of(ctx, f, &g);
    let p = plan(&g, &x);
    (g, p)
}

/// The quotient graph: the CFG with each outermost irreducible region one
/// node.
struct Quotient<'a> {
    exits: &'a [Vec<Exit>],
    /// Each CFG node's quotient node.
    q: Vec<usize>,
    /// Each quotient node's CFG node, or its region (blocks ascending,
    /// entries, and the distinct targets outside it in order).
    what: Vec<What>,
    rpo_ix: Vec<usize>,
    header: Vec<bool>,
    /// Edges into it that aren't retreating, counted per edge.
    forward_in: Vec<usize>,
    /// Its dominator tree children with two or more forward edges in,
    /// latest in reverse postorder first.
    merges: Vec<Vec<usize>>,
}

enum What {
    Node(usize),
    Region {
        blocks: Vec<usize>,
        entries: Vec<usize>,
        out: Vec<usize>,
    },
}

/// The plan, before its check.
pub fn plan_unchecked(g: &Cfg, exits: &[Vec<Exit>]) -> Result<Planned, Fail> {
    let n = g.len();
    let shape = Shape::of(g);
    let mut q = vec![usize::MAX; n];
    let mut what = Vec::new();
    let mut region_of = vec![None; n];
    let irr = shape.irreducible();
    for (i, r) in irr.iter().enumerate() {
        for &v in &r.blocks {
            region_of[v] = Some(i);
        }
    }
    let mut region_q = vec![usize::MAX; irr.len()];
    for v in 0..n {
        match region_of[v] {
            None => {
                q[v] = what.len();
                what.push(What::Node(v));
            }
            Some(i) if region_q[i] == usize::MAX => {
                region_q[i] = what.len();
                q[v] = what.len();
                let r = irr[i];
                let mut out = Vec::new();
                for &b in &r.blocks {
                    for x in &exits[b] {
                        for &t in &x.targets {
                            if region_of[t] != Some(i) && !out.contains(&t) {
                                out.push(t);
                            }
                        }
                    }
                }
                what.push(What::Region {
                    blocks: r.blocks.clone(),
                    entries: r.entries.clone(),
                    out,
                });
            }
            Some(i) => q[v] = region_q[i],
        }
    }
    // Each quotient node's edges, one per CFG edge (a region's: one per
    // target outside it).
    let qedges: Vec<Vec<usize>> = what
        .iter()
        .map(|w| match w {
            What::Node(v) => exits[*v]
                .iter()
                .flat_map(|x| x.targets.iter().map(|&t| q[t]))
                .collect(),
            What::Region { out, .. } => out.iter().map(|&t| q[t]).collect(),
        })
        .collect();
    let succ: Vec<Vec<usize>> = qedges
        .iter()
        .map(|e| {
            e.iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .collect();
    let qg = Cfg::new((0..what.len() as u32).collect(), succ);
    assert!(qg.unreachable.is_empty(), "a quotient node unreached");
    let qs = Shape::of(&qg);
    if !qs.reducible {
        return Err(Fail::Irreducible);
    }
    let m = what.len();
    let mut header = vec![false; m];
    let mut forward_in = vec![0; m];
    for (x, es) in qedges.iter().enumerate() {
        for &y in es {
            if qs.rpo_ix[y] <= qs.rpo_ix[x] {
                header[y] = true;
            } else {
                forward_in[y] += 1;
            }
        }
    }
    let mut merges = vec![Vec::new(); m];
    for &y in qs.rpo.iter().rev() {
        if y != 0 && forward_in[y] >= 2 {
            merges[qs.idom[y]].push(y);
        }
    }
    let quo = Quotient {
        exits,
        q,
        what,
        rpo_ix: qs.rpo_ix,
        header,
        forward_in,
        merges,
    };
    let mut tree = quo.tree(0);
    nexts(&mut tree, None);
    let regions = irr.len();
    let region_blocks = irr.iter().map(|r| r.blocks.len()).sum();
    Ok(Planned {
        tree,
        regions,
        region_blocks,
    })
}

impl Quotient<'_> {
    fn label(&self, y: usize) -> usize {
        match &self.what[y] {
            What::Node(v) => *v,
            What::Region { entries, .. } => entries[0],
        }
    }

    /// Ramsey's `doTree`.
    fn tree(&self, x: usize) -> Vec<Node> {
        let body = self.within(x, &self.merges[x]);
        if self.header[x] {
            vec![Node::Loop {
                label: self.label(x),
                body,
            }]
        } else {
            body
        }
    }

    /// Ramsey's `nodeWithin`: `x`'s code inside a labeled block for each
    /// of `ys`, the outermost first, each followed by its merge node.
    fn within(&self, x: usize, ys: &[usize]) -> Vec<Node> {
        let Some((&y, rest)) = ys.split_first() else {
            return vec![self.code(x)];
        };
        let mut out = vec![Node::Labeled {
            label: self.label(y),
            body: self.within(x, rest),
        }];
        out.extend(self.tree(y));
        out
    }

    fn code(&self, x: usize) -> Node {
        match &self.what[x] {
            &What::Node(v) => Node::Block {
                node: v,
                exits: self.exits[v]
                    .iter()
                    .map(|e| Out {
                        edges: e
                            .targets
                            .iter()
                            .map(|&t| Edge {
                                to: t,
                                how: self.branch(x, t),
                            })
                            .collect(),
                        fallback: e.leaves,
                    })
                    .collect(),
            },
            What::Region {
                blocks,
                entries,
                out,
            } => Node::Region {
                label: entries[0],
                entries: entries.clone(),
                blocks: blocks
                    .iter()
                    .map(|&v| Node::Block {
                        node: v,
                        exits: self.exits[v]
                            .iter()
                            .map(|e| Out {
                                edges: e
                                    .targets
                                    .iter()
                                    .map(|&t| Edge {
                                        to: t,
                                        how: if self.q[t] == x {
                                            How::Goto
                                        } else {
                                            How::Leave
                                        },
                                    })
                                    .collect(),
                                fallback: e.leaves,
                            })
                            .collect(),
                    })
                    .collect(),
                exits: out
                    .iter()
                    .map(|&t| Edge {
                        to: t,
                        how: self.branch(x, t),
                    })
                    .collect(),
            },
        }
    }

    /// Ramsey's `doBranch` for the edge from `x` to CFG node `t`.
    fn branch(&self, x: usize, t: usize) -> How {
        let y = self.q[t];
        if self.rpo_ix[y] <= self.rpo_ix[x] {
            How::Continue(self.label(y))
        } else if self.forward_in[y] >= 2 {
            How::Break(self.label(y))
        } else {
            How::Inline(self.tree(y))
        }
    }
}

/// Turn each `break` that ends the labeled block it leaves into a
/// fall-through. `fall` is the label whose merge code follows the end of
/// `seq`, if falling off it goes there.
fn nexts(seq: &mut [Node], fall: Option<usize>) {
    let len = seq.len();
    for (i, node) in seq.iter_mut().enumerate() {
        let last = i + 1 == len;
        match node {
            Node::Block { exits, .. } => {
                let n = exits.len();
                for (j, out) in exits.iter_mut().enumerate() {
                    let f = if last && j + 1 == n { fall } else { None };
                    edges_next(&mut out.edges, f);
                }
            }
            Node::Loop { body, .. } => nexts(body, None),
            Node::Labeled { label, body } => nexts(body, Some(*label)),
            Node::Region { exits, .. } => edges_next(exits, if last { fall } else { None }),
        }
    }
}

fn edges_next(edges: &mut [Edge], fall: Option<usize>) {
    for e in edges {
        match &mut e.how {
            How::Break(l) if Some(*l) == fall => e.how = How::Next,
            How::Inline(seq) => nexts(seq, fall),
            _ => {}
        }
    }
}

/// What a plan's tree says about its shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Depth {
    /// Loops, labeled blocks and regions a block sits in, at most: the
    /// plan's version of [`Shape::nesting`].
    pub nesting: usize,
    /// Those and the `if` and `match` arms an inlined edge sits in: how
    /// deep the printed Rust nests.
    pub indent: usize,
}

impl Planned {
    pub fn depth(&self) -> Depth {
        let mut d = Depth::default();
        depth(&self.tree, 0, 0, &mut d);
        d
    }
}

fn depth(seq: &[Node], nesting: usize, indent: usize, d: &mut Depth) {
    for node in seq {
        match node {
            Node::Block { exits, .. } => {
                d.nesting = d.nesting.max(nesting);
                d.indent = d.indent.max(indent);
                let n = exits.len();
                for (j, out) in exits.iter().enumerate() {
                    let arm = j + 1 < n || out.edges.len() > 1;
                    for e in &out.edges {
                        if let How::Inline(s) = &e.how {
                            depth(s, nesting, indent + arm as usize, d);
                        }
                    }
                }
            }
            Node::Loop { body, .. } | Node::Labeled { body, .. } => {
                depth(body, nesting + 1, indent + 1, d)
            }
            Node::Region { blocks, exits, .. } => {
                depth(blocks, nesting + 1, indent + 2, d);
                for e in exits {
                    if let How::Inline(s) = &e.how {
                        depth(s, nesting, indent + 1, d);
                    }
                }
            }
        }
    }
}

/// The structure check: `tree` places every node of `g` exactly once,
/// starts at the entry, gives each node's exits their targets in order
/// with their fallbacks, and sends every edge to its own target through a
/// label in scope.
pub fn check(g: &Cfg, exits: &[Vec<Exit>], tree: &[Node]) -> Result<(), String> {
    let mut c = Checker {
        exits,
        seen: vec![false; g.len()],
        scopes: Vec::new(),
    };
    if !c.starts(tree)?.contains(&0) {
        return Err("the plan doesn't start at the entry".into());
    }
    c.seq(tree, None, None)?;
    if let Some(v) = c.seen.iter().position(|s| !s) {
        return Err(format!("node {v} not placed"));
    }
    Ok(())
}

enum Scope {
    Loop(usize, Vec<usize>),
    Labeled(usize, Vec<usize>),
}

/// A region's blocks and the targets its exits have arms for.
struct InRegion {
    blocks: BTreeSet<usize>,
    out: BTreeSet<usize>,
}

struct Checker<'a> {
    exits: &'a [Vec<Exit>],
    seen: Vec<bool>,
    scopes: Vec<Scope>,
}

impl Checker<'_> {
    /// The nodes the code of `seq` may start at.
    fn starts(&self, seq: &[Node]) -> Result<Vec<usize>, String> {
        match seq.first() {
            None => Err("an empty sequence".into()),
            Some(Node::Block { node, .. }) => Ok(vec![*node]),
            Some(Node::Loop { body, .. } | Node::Labeled { body, .. }) => self.starts(body),
            Some(Node::Region { entries, .. }) => Ok(entries.clone()),
        }
    }

    /// Check `seq`; falling off its end starts one of `fall`, if it may.
    fn seq(
        &mut self,
        seq: &[Node],
        fall: Option<&[usize]>,
        region: Option<&InRegion>,
    ) -> Result<(), String> {
        for (i, node) in seq.iter().enumerate() {
            let after = match seq.get(i + 1) {
                Some(_) => Some(self.starts(&seq[i + 1..])?),
                None => fall.map(<[usize]>::to_vec),
            };
            match node {
                Node::Block { node, exits } => {
                    let v = *node;
                    if v >= self.seen.len() || std::mem::replace(&mut self.seen[v], true) {
                        return Err(format!("node {v} placed twice or unknown"));
                    }
                    let want = &self.exits[v];
                    if exits.len() != want.len() {
                        return Err(format!(
                            "node {v}: {} exits for {}",
                            exits.len(),
                            want.len()
                        ));
                    }
                    let n = exits.len();
                    for (j, (out, x)) in exits.iter().zip(want).enumerate() {
                        let to: Vec<usize> = out.edges.iter().map(|e| e.to).collect();
                        if to != x.targets {
                            return Err(format!(
                                "node {v} exit {j}: targets {to:?}, not {:?}",
                                x.targets
                            ));
                        }
                        if out.fallback != x.leaves {
                            return Err(format!("node {v} exit {j}: fallback changed"));
                        }
                        // Only the last exit's edges fall into what follows.
                        let f = if j + 1 == n { after.as_deref() } else { None };
                        for e in &out.edges {
                            self.edge(e, f, region)
                                .map_err(|m| format!("node {v} exit {j} to {}: {m}", e.to))?;
                        }
                    }
                }
                Node::Loop { label, body } => {
                    let s = self.starts(body)?;
                    self.scopes.push(Scope::Loop(*label, s));
                    self.seq(body, None, region)?;
                    self.scopes.pop();
                }
                Node::Labeled { label, body } => {
                    let Some(a) = after.clone() else {
                        return Err(format!("labeled block {label} with nothing after it"));
                    };
                    self.scopes.push(Scope::Labeled(*label, a.clone()));
                    self.seq(body, Some(&a), region)?;
                    self.scopes.pop();
                }
                Node::Region {
                    entries,
                    blocks,
                    exits,
                    ..
                } => {
                    if region.is_some() {
                        return Err("a region inside a region".into());
                    }
                    let mut inside = BTreeSet::new();
                    for b in blocks {
                        match b {
                            Node::Block { node, .. } => inside.insert(*node),
                            _ => return Err("a region holding more than blocks".into()),
                        };
                    }
                    if entries.is_empty() || entries.iter().any(|e| !inside.contains(e)) {
                        return Err(format!("region entries {entries:?} not its blocks"));
                    }
                    let out: BTreeSet<usize> = exits.iter().map(|e| e.to).collect();
                    if out.len() != exits.len() || out.iter().any(|t| inside.contains(t)) {
                        return Err("a region's exit arms repeat or lead inside".into());
                    }
                    let r = InRegion {
                        blocks: inside,
                        out,
                    };
                    let scopes = std::mem::take(&mut self.scopes);
                    let inner = blocks
                        .iter()
                        .try_for_each(|b| self.seq(std::slice::from_ref(b), None, Some(&r)));
                    self.scopes = scopes;
                    inner?;
                    for e in exits {
                        self.edge(e, after.as_deref(), None)
                            .map_err(|m| format!("region exit to {}: {m}", e.to))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn edge(
        &mut self,
        e: &Edge,
        fall: Option<&[usize]>,
        region: Option<&InRegion>,
    ) -> Result<(), String> {
        if let Some(r) = region {
            return match e.how {
                How::Goto if r.blocks.contains(&e.to) => Ok(()),
                How::Leave if r.out.contains(&e.to) => Ok(()),
                _ => Err("not a goto inside its region or a leave to one of its arms".into()),
            };
        }
        let reaches = |s: &[usize]| s.contains(&e.to);
        match &e.how {
            How::Continue(l) => match self.scopes.iter().rev().find_map(|s| match s {
                Scope::Loop(m, s) if m == l => Some(s),
                _ => None,
            }) {
                Some(s) if reaches(s) => Ok(()),
                Some(_) => Err(format!("continue {l} doesn't start there")),
                None => Err(format!("no loop {l} around it")),
            },
            How::Break(l) => match self.scopes.iter().rev().find_map(|s| match s {
                Scope::Labeled(m, s) if m == l => Some(s),
                _ => None,
            }) {
                Some(s) if reaches(s) => Ok(()),
                Some(_) => Err(format!("break {l} doesn't lead there")),
                None => Err(format!("no labeled block {l} around it")),
            },
            How::Next => match fall {
                Some(s) if reaches(s) => Ok(()),
                _ => Err("falls into something else".into()),
            },
            How::Inline(seq) => {
                if !reaches(&self.starts(seq)?) {
                    return Err("inlined code starts elsewhere".into());
                }
                self.seq(seq, fall, None)
            }
            How::Goto | How::Leave => Err("a goto or leave outside a region".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CFG of nodes `0..exits.len()` whose exits have these targets;
    /// an exit with none leaves.
    fn graph(exits: &[&[&[usize]]]) -> (Cfg, Vec<Vec<Exit>>) {
        let xs: Vec<Vec<Exit>> = exits
            .iter()
            .map(|b| {
                b.iter()
                    .map(|t| Exit {
                        targets: t.to_vec(),
                        leaves: t.is_empty(),
                    })
                    .collect()
            })
            .collect();
        let succ = xs
            .iter()
            .map(|b| {
                let s: BTreeSet<usize> = b.iter().flat_map(|x| x.targets.iter().copied()).collect();
                s.into_iter().collect()
            })
            .collect();
        let g = Cfg::new((0..exits.len() as u32).collect(), succ);
        assert!(g.unreachable.is_empty());
        (g, xs)
    }

    fn block(node: usize, exits: Vec<Vec<Edge>>) -> Node {
        Node::Block {
            node,
            exits: exits
                .into_iter()
                .map(|edges| Out {
                    fallback: edges.is_empty(),
                    edges,
                })
                .collect(),
        }
    }

    fn ret(node: usize) -> Node {
        block(node, vec![vec![]])
    }

    fn e(to: usize, how: How) -> Edge {
        Edge { to, how }
    }

    #[test]
    fn diamond() {
        // 0 branches to 1 or 2, both go to 3.
        let (g, x) = graph(&[&[&[1], &[2]], &[&[3]], &[&[3]], &[&[]]]);
        let p = plan(&g, &x).unwrap();
        let want = vec![
            Node::Labeled {
                label: 3,
                body: vec![block(
                    0,
                    vec![
                        vec![e(
                            1,
                            How::Inline(vec![block(1, vec![vec![e(3, How::Break(3))]])]),
                        )],
                        vec![e(
                            2,
                            How::Inline(vec![block(2, vec![vec![e(3, How::Next)]])]),
                        )],
                    ],
                )],
            },
            ret(3),
        ];
        assert_eq!(p.tree, want);
        assert_eq!(
            p.depth(),
            Depth {
                nesting: 1,
                indent: 2
            }
        );
    }

    #[test]
    fn wait_for_vblank() {
        // The design's example: 0 skips to 2 or enters the spin at 1.
        let (g, x) = graph(&[&[&[2], &[1]], &[&[1], &[2]], &[&[]]]);
        let p = plan(&g, &x).unwrap();
        let spin = Node::Loop {
            label: 1,
            body: vec![block(
                1,
                vec![vec![e(1, How::Continue(1))], vec![e(2, How::Break(2))]],
            )],
        };
        let want = vec![
            Node::Labeled {
                label: 2,
                body: vec![block(
                    0,
                    vec![
                        vec![e(2, How::Break(2))],
                        vec![e(1, How::Inline(vec![spin]))],
                    ],
                )],
            },
            ret(2),
        ];
        assert_eq!(p.tree, want);
    }

    #[test]
    fn self_loop_at_the_entry() {
        let (g, x) = graph(&[&[&[0], &[1]], &[&[]]]);
        let p = plan(&g, &x).unwrap();
        let want = vec![Node::Loop {
            label: 0,
            body: vec![block(
                0,
                vec![
                    vec![e(0, How::Continue(0))],
                    vec![e(1, How::Inline(vec![ret(1)]))],
                ],
            )],
        }];
        assert_eq!(p.tree, want);
    }

    #[test]
    fn nested_loops_and_several_exits() {
        // 1 heads a loop with 2, which loops on itself and leaves to 4 as
        // well as back to 1; 1 leaves to 3, which goes on to 4.
        let (g, x) = graph(&[&[&[1]], &[&[3], &[2]], &[&[2], &[4], &[1]], &[&[4]], &[&[]]]);
        let p = plan(&g, &x).unwrap();
        check(&g, &x, &p.tree).unwrap();
        assert_eq!(p.depth().nesting, 3);
        assert_eq!(p.regions, 0);
    }

    #[test]
    fn switch_with_shared_targets() {
        // 0's table goes to 1, 2 or 3 (or the dispatcher); 1 and 2 go to 3.
        let (g, mut x) = graph(&[&[&[1, 2, 3]], &[&[3]], &[&[3]], &[&[]]]);
        x[0][0].leaves = true;
        let p = plan(&g, &x).unwrap();
        let want = vec![
            Node::Labeled {
                label: 3,
                body: vec![Node::Block {
                    node: 0,
                    exits: vec![Out {
                        edges: vec![
                            e(1, How::Inline(vec![block(1, vec![vec![e(3, How::Next)]])])),
                            e(2, How::Inline(vec![block(2, vec![vec![e(3, How::Next)]])])),
                            e(3, How::Next),
                        ],
                        fallback: true,
                    }],
                }],
            },
            ret(3),
        ];
        assert_eq!(p.tree, want);
        assert_eq!(p.depth().indent, 2);
    }

    #[test]
    fn two_exits_to_one_target_merge() {
        // 0's conditional exit and its fallthrough both go to 1.
        let (g, x) = graph(&[&[&[1], &[1]], &[&[]]]);
        let p = plan(&g, &x).unwrap();
        let want = vec![
            Node::Labeled {
                label: 1,
                body: vec![block(
                    0,
                    vec![vec![e(1, How::Break(1))], vec![e(1, How::Next)]],
                )],
            },
            ret(1),
        ];
        assert_eq!(p.tree, want);
    }

    #[test]
    fn irreducible_region_is_collapsed() {
        // 1 and 2 jump to each other and the entry reaches both; 2 leaves
        // to 3.
        let (g, x) = graph(&[&[&[1], &[2]], &[&[2]], &[&[1], &[3]], &[&[]]]);
        let p = plan(&g, &x).unwrap();
        let region = Node::Region {
            label: 1,
            entries: vec![1, 2],
            blocks: vec![
                block(1, vec![vec![e(2, How::Goto)]]),
                block(2, vec![vec![e(1, How::Goto)], vec![e(3, How::Leave)]]),
            ],
            exits: vec![e(3, How::Inline(vec![ret(3)]))],
        };
        let want = vec![
            Node::Labeled {
                label: 1,
                body: vec![block(
                    0,
                    vec![vec![e(1, How::Break(1))], vec![e(2, How::Next)]],
                )],
            },
            region,
        ];
        assert_eq!(p.tree, want);
        assert_eq!((p.regions, p.region_blocks), (1, 2));
    }

    #[test]
    fn irreducible_region_in_a_loop() {
        // A loop at 1 whose body 2/3 is entered at both; 3 continues the
        // loop, 1 leaves to 4.
        let (g, x) = graph(&[&[&[1]], &[&[2], &[3], &[4]], &[&[3]], &[&[2], &[1]], &[&[]]]);
        let p = plan(&g, &x).unwrap();
        assert_eq!(p.regions, 1);
        // The region sits in loop 1, and its exit back to the header is a
        // continue.
        let found = format!("{:?}", p.tree);
        assert!(found.contains("Loop { label: 1"), "{found}");
        assert!(found.contains("Region { label: 2"), "{found}");
        assert!(found.contains("to: 1, how: Continue(1)"), "{found}");
    }

    #[test]
    fn loop_at_the_entry_around_a_region() {
        // The entry heads a loop through 1 and 2, which also jump to each
        // other: with the edges back to 0 cut, 1/2 is entered at both.
        let (g, x) = graph(&[&[&[1], &[2]], &[&[0], &[2]], &[&[1], &[3]], &[&[]]]);
        let p = plan(&g, &x).unwrap();
        assert_eq!((p.regions, p.region_blocks), (1, 2));
        assert!(
            matches!(&p.tree[..], [Node::Loop { label: 0, .. }]),
            "{:?}",
            p.tree
        );
    }

    #[test]
    fn check_catches_broken_plans() {
        let (g, x) = graph(&[&[&[1], &[2]], &[&[3]], &[&[3]], &[&[]]]);
        let good = plan(&g, &x).unwrap().tree;
        let broken = |f: &dyn Fn(&mut Vec<Node>)| {
            let mut t = good.clone();
            f(&mut t);
            check(&g, &x, &t).unwrap_err()
        };
        // Node 3 twice.
        let m = broken(&|t| t.push(ret(3)));
        assert!(m.contains("twice"), "{m}");
        // Node 3 missing: nothing after the labeled block.
        let m = broken(&|t| {
            t.pop();
        });
        assert!(m.contains("nothing after"), "{m}");
        // A break to a label not in scope.
        let m = broken(&|t| {
            let Node::Labeled { label, .. } = &mut t[0] else {
                panic!()
            };
            *label = 7;
        });
        assert!(m.contains("no labeled block 3"), "{m}");
        // The table's fallback dropped.
        let m = broken(&|t| {
            let Node::Block { exits, .. } = t.last_mut().unwrap() else {
                panic!()
            };
            exits[0].fallback = false;
        });
        assert!(m.contains("fallback"), "{m}");
        // An edge sent to the wrong target.
        let m = broken(&|t| {
            let Node::Labeled { body, .. } = &mut t[0] else {
                panic!()
            };
            let Node::Block { exits, .. } = &mut body[0] else {
                panic!()
            };
            exits[0].edges[0].to = 2;
        });
        assert!(m.contains("targets"), "{m}");
        // A conditional exit's inlined code falling into the merge.
        let m = broken(&|t| {
            let Node::Labeled { body, .. } = &mut t[0] else {
                panic!()
            };
            let Node::Block { exits, .. } = &mut body[0] else {
                panic!()
            };
            let How::Inline(s) = &mut exits[0].edges[0].how else {
                panic!()
            };
            let Node::Block { exits, .. } = &mut s[0] else {
                panic!()
            };
            exits[0].edges[0].how = How::Next;
        });
        assert!(m.contains("falls into"), "{m}");
        // Starting anywhere but the entry.
        let m = broken(&|t| t.reverse());
        assert!(m.contains("entry"), "{m}");
    }

    #[test]
    fn long_chain_of_merges() {
        // 0 → 1..n each a diamond in sequence: deep but checked.
        let n = 300;
        let mut ex: Vec<Vec<Vec<usize>>> = Vec::new();
        for i in 0..n {
            let b = 3 * i;
            ex.push(vec![vec![b + 1], vec![b + 2]]);
            ex.push(vec![vec![b + 3]]);
            ex.push(vec![vec![b + 3]]);
        }
        ex.push(vec![vec![]]);
        let refs: Vec<Vec<&[usize]>> = ex
            .iter()
            .map(|b| b.iter().map(Vec::as_slice).collect())
            .collect();
        let refs: Vec<&[&[usize]]> = refs.iter().map(Vec::as_slice).collect();
        let (g, x) = graph(&refs);
        let p = plan(&g, &x).unwrap();
        // Each merge follows its labeled block: no nesting builds up.
        assert_eq!(p.depth().nesting, 1);
    }
}
