use super::*;

#[test]
fn fusion_rewards_agreement_and_breaks_ties_by_key() {
    let a = vec![("x".into(), 1.0), ("y".into(), 0.5)];
    let b = vec![("y".into(), 0.9), ("x".into(), 0.1)];
    let fused = fuse(&[(1.0, a.clone()), (1.0, b.clone())]);
    assert_eq!(fused[0].0.as_str(), "x");
    assert_eq!(fused[1].0.as_str(), "y");
    // A list's weight multiplies what it adds: three times `b` puts its
    // first before `a`'s, at exactly the weighted sum.
    let weighed = fuse(&[(1.0, a), (3.0, b)]);
    assert_eq!(weighed[0], ("y".into(), 1.0 / 61.0 + 3.0 / 60.0));
    assert_eq!(weighed[1], ("x".into(), 1.0 / 60.0 + 3.0 / 61.0));
    let only = fuse(&[
        (1.0, vec![("b".into(), 1.0)]),
        (1.0, vec![("a".into(), 1.0)]),
    ]);
    assert_eq!(
        only.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn a_cursor_round_trips_and_a_foreign_one_is_refused() {
    let c = encode(&CursorState {
        at: Position(7),
        after: Some((vec![Value::from(1)], "k".into())),
        offset: 0,
        params: Default::default(),
        door: "find".into(),
        ty: Some("note".into()),
        of: String::new(),
        now: Some(Timestamp(5)),
        stage: None,
        unranked: None,
    });
    let back = decode(&c).unwrap();
    assert_eq!((back.at, back.now), (Position(7), Some(Timestamp(5))));
    assert!(decode(&Cursor("zz".into())).is_err());
    assert!(decode(&Cursor("7b7d".into())).is_err());
}

#[test]
fn only_filters_an_engine_can_apply_alone_are_handed_down() {
    let parse = |t: &str| lakelands_core::expr::parse(t).unwrap();
    assert!(!needs_store(&parse("amount > 10 and status != 'paid'")));
    for kept in [
        "due < now()",
        "rate = setting('vat_rate')",
        "known_at(amount) > due",
        "matches(number, 'INV-[0-9]+')",
        "not (matches(number, 'X') or amount > 1)",
    ] {
        assert!(needs_store(&parse(kept)), "`{kept}` was handed down");
    }
}

/// A read takes its clock in [`Reader::open`] alone (lakelands#571): of
/// the uses of the wall clock in the store's and the core's product code,
/// the one in `reads.rs` is that decision's, inside `open`, and each other
/// is a write's or a backing's. A use added anywhere is a decision of its
/// own, and moves this table.
#[test]
fn only_a_reads_open_reads_the_wall_clock() {
    let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [
        ("store", crate_dir.join("src")),
        ("core", crate_dir.join("../lakelands-core/src")),
    ];
    // Each token, bare: a call, or the function handed on by name.
    let uses = |l: &str| {
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        ["wall_clock", "SystemTime::now", "UNIX_EPOCH"]
            .iter()
            .flat_map(|w| l.match_indices(w).map(move |(i, _)| (i, i + w.len())))
            .filter(|&(i, j)| !l[..i].ends_with(ident) && !l[j..].starts_with(ident))
            .count()
    };
    let mut found = BTreeMap::new();
    for (name, root) in &roots {
        let mut dirs = vec![root.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let file = path.file_name().unwrap().to_string_lossy();
                if path.is_dir() {
                    if file != "conformance" {
                        dirs.push(path);
                    }
                    continue;
                }
                if file == "tests.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let lines: Vec<&str> = text.lines().collect();
                let rel = path.strip_prefix(root).unwrap().to_string_lossy();
                let key = format!("{name}/{}", rel.replace('\\', "/"));
                if key.starts_with("store/reads") || key == "store/doors.rs" {
                    // A read takes no writer's clock either: it would be
                    // the wall clock by another name.
                    let writer = lines.iter().position(|l| {
                        !l.trim_start().starts_with("//")
                            && (l.contains(".clock(") || l.contains(".reading("))
                    });
                    assert_eq!(writer, None, "{key} reads a writer's clock");
                }
                let at: Vec<usize> = (0..lines.len())
                    .filter(|&i| !lines[i].trim_start().starts_with("//"))
                    .filter(|&i| !lines[i].trim_start().starts_with("use "))
                    .filter(|&i| !lines[i].contains("fn wall_clock"))
                    .flat_map(|i| std::iter::repeat_n(i, uses(lines[i])))
                    .collect();
                if at.is_empty() {
                    continue;
                }
                if key == "store/reads.rs" {
                    // Each use lies inside `Reader::open`'s body.
                    let open = lines
                        .iter()
                        .position(|l| l.trim() == "pub fn open(")
                        .unwrap();
                    let end = (open..lines.len()).find(|&i| lines[i] == "    }").unwrap();
                    assert!(
                        at.iter().all(|&i| open < i && i < end),
                        "{at:?} outside open"
                    );
                }
                found.insert(key, at.len());
            }
        }
    }
    let expected: BTreeMap<String, usize> = [
        // The expression backing's own call, outside any read or fold.
        ("store/backing.rs", 1),
        // The wall clock itself (its two names), a writer's first clock,
        // and a draft's stamp.
        ("store/lake.rs", 4),
        // `Reader::open`: a read at the head.
        ("store/reads.rs", 1),
        // A write's clock, never before the last `known_at`.
        ("store/writes/propose.rs", 1),
    ]
    .map(|(f, n)| (f.to_owned(), n))
    .into();
    assert_eq!(found, expected);
}
