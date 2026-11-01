use super::*;

fn fun(key: u32, insns: usize, bytes: u64, toks: &[Tok], calls: &[u32]) -> Fun {
    let mut t = BTreeMap::new();
    for &k in toks {
        *t.entry(k).or_insert(0) += 1;
    }
    Fun {
        key,
        insns,
        bytes,
        calls: calls.iter().copied().collect(),
        refs: BTreeSet::new(),
        ram: BTreeSet::new(),
        toks: t,
    }
}

/// Filler both sides share, so that a token on a few functions is rare.
fn filler(base: u32, n: u32) -> Vec<Fun> {
    (0..n)
        .map(|i| {
            fun(
                base + 0x100 * i,
                30,
                1000 + i as u64,
                &[Tok::Const(0x10_0000 + i)],
                &[],
            )
        })
        .collect()
}

fn pairs_by_key(a: &Side, b: &Side, ps: &[Pair]) -> BTreeMap<u32, u32> {
    ps.iter()
        .map(|p| (b.funs[p.b].key, a.funs[p.a].key))
        .collect()
}

/// The same bytes on one function each side pair up; bytes two A
/// functions share pair nothing.
#[test]
fn unique_bytes_anchor() {
    let mut fa = filler(0x0810_0000, 20);
    fa.push(fun(0x0800_0001, 10, 7, &[], &[]));
    fa.push(fun(0x0800_0101, 10, 8, &[], &[]));
    fa.push(fun(0x0800_0201, 10, 8, &[], &[]));
    let mut fb = filler(0x0910_0000, 20);
    fb.push(fun(0x0880_0001, 10, 7, &[], &[]));
    fb.push(fun(0x0880_0101, 10, 8, &[], &[]));
    let (a, b) = (Side::new(fa), Side::new(fb));
    let cfg = Config {
        spread: false,
        ..Config::default()
    };
    let ps = Matcher::new(&a, &b, cfg).run();
    let by = pairs_by_key(&a, &b, &ps);
    assert_eq!(by.get(&0x0880_0001), Some(&0x0800_0001));
    assert_eq!(by.get(&0x0880_0101), None);
    let p = ps.iter().find(|p| b.funs[p.b].key == 0x0880_0001).unwrap();
    assert_eq!(p.stage, Stage::Bytes);
}

/// Rare tokens pair a function with its counterpart; one with two equally
/// good candidates stays unmatched.
#[test]
fn fingerprints_pair_and_ties_stay_apart() {
    let io = Tok::Hw(0x0400_0208);
    let swi = Tok::Swi(0x0b);
    let mut fa = filler(0x0810_0000, 40);
    fa.push(fun(0x0800_0001, 40, 1, &[io, swi, Tok::Const(0x1234)], &[]));
    fa.push(fun(0x0800_0101, 20, 2, &[Tok::Const(0x777)], &[]));
    fa.push(fun(0x0800_0201, 20, 3, &[Tok::Const(0x777)], &[]));
    let mut fb = filler(0x0910_0000, 40);
    fb.push(fun(
        0x0880_0001,
        48,
        11,
        &[io, swi, Tok::Const(0x1234)],
        &[],
    ));
    fb.push(fun(0x0880_0101, 20, 12, &[Tok::Const(0x777)], &[]));
    let (a, b) = (Side::new(fa), Side::new(fb));
    let ps = Matcher::new(&a, &b, Config::default()).run();
    let by = pairs_by_key(&a, &b, &ps);
    assert_eq!(by.get(&0x0880_0001), Some(&0x0800_0001));
    assert_eq!(by.get(&0x0880_0101), None, "two A candidates tie");
}

/// A function whose own tokens are ambiguous is matched once its callee
/// is: the matched callee becomes a token.
#[test]
fn spreading_through_a_matched_callee() {
    let leaf = [Tok::Hw(0x0400_0004), Tok::Const(0x4321)];
    let common = [Tok::Const(0x99)];
    let mut fa = filler(0x0810_0000, 40);
    fa.push(fun(0x0800_0001, 30, 1, &leaf, &[]));
    fa.push(fun(0x0800_0101, 20, 2, &common, &[0x0800_0001]));
    fa.push(fun(0x0800_0201, 20, 3, &common, &[]));
    let mut fb = filler(0x0910_0000, 40);
    fb.push(fun(0x0880_0001, 30, 11, &leaf, &[]));
    fb.push(fun(0x0880_0101, 20, 12, &common, &[0x0880_0001]));
    let (a, b) = (Side::new(fa), Side::new(fb));
    let alone = Matcher::new(
        &a,
        &b,
        Config {
            spread: false,
            ..Config::default()
        },
    )
    .run();
    assert_eq!(pairs_by_key(&a, &b, &alone).get(&0x0880_0101), None);
    let ps = Matcher::new(&a, &b, Config::default()).run();
    let by = pairs_by_key(&a, &b, &ps);
    assert_eq!(by.get(&0x0880_0001), Some(&0x0800_0001));
    assert_eq!(by.get(&0x0880_0101), Some(&0x0800_0101));
}

/// The score counts each verdict in its size class, unnamed pairs left out
/// of precision.
#[test]
fn score_counts_verdicts() {
    let a = Side::new(vec![
        fun(0x0800_0001, 10, 1, &[], &[]),
        fun(0x0800_0101, 50, 2, &[], &[]),
        fun(0x0800_0201, 200, 3, &[], &[]),
    ]);
    let b = Side::new(vec![
        fun(0x0880_0001, 10, 1, &[], &[]),
        fun(0x0880_0101, 50, 2, &[], &[]),
        fun(0x0880_0201, 400, 3, &[], &[]),
    ]);
    let p = |a, b| Pair {
        a,
        b,
        score: 1.0,
        stage: Stage::Print,
        round: 0,
    };
    let truth: BTreeMap<usize, usize> = [(0, 0), (2, 2)].into();
    let pairs = [p(0, 0), p(1, 1), p(2, 2)];
    let s = score(&a, &b, &pairs, &truth, |x, y| match (x, y) {
        (1, 1) => Verdict::Unnamed,
        (x, y) if truth.get(&y) == Some(&x) => Verdict::Right,
        _ => Verdict::Wrong,
    });
    assert_eq!(s.matchable, [1, 0, 1]);
    assert_eq!(s.made, [1, 1, 1]);
    assert_eq!(s.right, [1, 0, 1]);
    assert_eq!(s.unnamed, [0, 1, 0]);
    assert_eq!(s.judged_all(), 2);
    assert_eq!(s.precision(), 1.0);
    assert_eq!(s.recall(), 1.0);
    // 200 against 400 instructions: inlining.
    assert_eq!(s.resized, 1);
    assert_eq!(class(15), 0);
    assert_eq!(class(16), 1);
    assert_eq!(class(101), 2);
}
