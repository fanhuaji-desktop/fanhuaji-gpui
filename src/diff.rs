//! Line diff for the preview pane, ported from the Tauri app's `diff.ts` / `inlineDiff`.

use std::ops::Range;

/// One side of a changed line; `changed` holds byte ranges into `text`.
#[derive(Debug, PartialEq)]
pub struct Marked {
    pub text: String,
    pub changed: Vec<Range<usize>>,
}

#[derive(Debug, PartialEq)]
pub enum Row {
    Equal(String),
    Replace { orig: Marked, conv: Marked },
    Insert(String),
    Delete(String),
}

/// Above this many DP cells, fall back to index alignment (e.g. a file of 1-char lines).
const LINE_LCS_CELLS: usize = 4_000_000;
/// Longest changed middle (in chars) that still gets a char-level LCS.
const INLINE_LCS_LIMIT: usize = 300;

/// LCS table: `dp[i][j]` is the LCS length of `a[i..]` and `b[j..]`.
fn lcs_table<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Vec<u32>> {
    let mut dp = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    dp
}

/// Line-level LCS diff. zhconvert prepends a watermark block to subtitle output; LCS
/// treats it as an insertion so the rest stays aligned. Runs of deletes followed by
/// inserts are paired up as char-highlighted replaces.
pub fn diff_lines(original: &str, converted: &str) -> Vec<Row> {
    let o: Vec<&str> = original.split('\n').collect();
    let c: Vec<&str> = converted.split('\n').collect();
    let mut rows = Vec::new();

    if o.len() * c.len() > LINE_LCS_CELLS {
        for i in 0..o.len().max(c.len()) {
            let a = o.get(i).copied().unwrap_or("");
            let b = c.get(i).copied().unwrap_or("");
            rows.push(if a == b {
                Row::Equal(b.into())
            } else {
                replace(a, b)
            });
        }
        return rows;
    }

    let dp = lcs_table(&o, &c);
    let (mut dels, mut inss): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    let flush = |rows: &mut Vec<Row>, dels: &mut Vec<&str>, inss: &mut Vec<&str>| {
        for p in 0..dels.len().max(inss.len()) {
            rows.push(match (dels.get(p), inss.get(p)) {
                (Some(a), Some(b)) => replace(a, b),
                (None, Some(b)) => Row::Insert((*b).into()),
                (Some(a), None) => Row::Delete((*a).into()),
                (None, None) => unreachable!(),
            });
        }
        dels.clear();
        inss.clear();
    };

    let (mut i, mut j) = (0, 0);
    while i < o.len() && j < c.len() {
        if o[i] == c[j] {
            flush(&mut rows, &mut dels, &mut inss);
            rows.push(Row::Equal(c[j].into()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            dels.push(o[i]);
            i += 1;
        } else {
            inss.push(c[j]);
            j += 1;
        }
    }
    dels.extend(&o[i..]);
    inss.extend(&c[j..]);
    flush(&mut rows, &mut dels, &mut inss);
    rows
}

fn replace(a: &str, b: &str) -> Row {
    let (oc, cc) = inline_diff(a, b);
    Row::Replace {
        orig: Marked {
            text: a.into(),
            changed: oc,
        },
        conv: Marked {
            text: b.into(),
            changed: cc,
        },
    }
}

/// Byte ranges that changed on each side. Shared prefix/suffix are trimmed; equal-length
/// middles (the usual one-for-one swap) compare by position, otherwise a char LCS is
/// used while the middles are short enough, else the whole middle is marked.
pub fn inline_diff(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let ac: Vec<(usize, char)> = a.char_indices().collect();
    let bc: Vec<(usize, char)> = b.char_indices().collect();
    let mut p = 0;
    while p < ac.len() && p < bc.len() && ac[p].1 == bc[p].1 {
        p += 1;
    }
    let mut s = 0;
    while s < ac.len() - p && s < bc.len() - p && ac[ac.len() - 1 - s].1 == bc[bc.len() - 1 - s].1 {
        s += 1;
    }
    let am = &ac[p..ac.len() - s];
    let bm = &bc[p..bc.len() - s];

    let (ak, bk): (Vec<bool>, Vec<bool>) = if am.len() == bm.len() {
        let same: Vec<bool> = am.iter().zip(bm).map(|(x, y)| x.1 == y.1).collect();
        (same.clone(), same)
    } else if am.len() <= INLINE_LCS_LIMIT && bm.len() <= INLINE_LCS_LIMIT {
        let x: Vec<char> = am.iter().map(|c| c.1).collect();
        let y: Vec<char> = bm.iter().map(|c| c.1).collect();
        let dp = lcs_table(&x, &y);
        let (mut xk, mut yk) = (vec![false; x.len()], vec![false; y.len()]);
        let (mut i, mut j) = (0, 0);
        while i < x.len() && j < y.len() {
            if x[i] == y[j] {
                xk[i] = true;
                yk[j] = true;
                i += 1;
                j += 1;
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
        (xk, yk)
    } else {
        (vec![false; am.len()], vec![false; bm.len()])
    };
    (ranges(am, &ak), ranges(bm, &bk))
}

/// Merge consecutive changed chars into byte ranges.
fn ranges(chars: &[(usize, char)], kept: &[bool]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (&(start, ch), &k) in chars.iter().zip(kept) {
        if k {
            continue;
        }
        let end = start + ch.len_utf8();
        match out.last_mut() {
            Some(r) if r.end == start => r.end = end,
            _ => out.push(start..end),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    //! Cases ported one-for-one from the Tauri app's `diff.test.ts` and `inlineDiff` tests.
    use super::*;

    fn changed(a: &str, b: &str) -> (Vec<String>, Vec<String>) {
        let (o, c) = inline_diff(a, b);
        let pick =
            |s: &str, rs: Vec<Range<usize>>| rs.into_iter().map(|r| s[r].to_string()).collect();
        (pick(a, o), pick(b, c))
    }

    fn rep(orig: &str, conv: &str) -> Row {
        replace(orig, conv)
    }

    fn lines(v: &[String]) -> String {
        v.join("\n")
    }

    #[test]
    fn keeps_lines_aligned_after_an_inserted_watermark() {
        assert_eq!(
            diff_lines("1\n软件", "0\n繁化姬\n1\n軟體"),
            vec![
                Row::Insert("0".into()),
                Row::Insert("繁化姬".into()),
                Row::Equal("1".into()),
                rep("软件", "軟體"),
            ]
        );
    }

    #[test]
    fn pairs_deletes_with_inserts_and_leaves_the_rest_unpaired() {
        assert_eq!(
            diff_lines("a\nb\nc", "x"),
            vec![
                rep("a", "x"),
                Row::Delete("b".into()),
                Row::Delete("c".into())
            ]
        );
    }

    #[test]
    fn marks_a_line_added_in_the_middle_as_an_insert() {
        assert_eq!(
            diff_lines("a\nc", "a\nb\nc"),
            vec![
                Row::Equal("a".into()),
                Row::Insert("b".into()),
                Row::Equal("c".into())
            ]
        );
    }

    #[test]
    fn pads_the_shorter_side_in_the_large_input_fallback() {
        let o: Vec<String> = (0..2001).map(|i| format!("o{i}")).collect();
        let c: Vec<String> = (0..2002).map(|i| format!("o{i}")).collect();
        let rows = diff_lines(&lines(&o), &lines(&c));
        assert_eq!(rows.len(), 2002);
        assert_eq!(rows[2001], rep("", "o2001"));
        assert_eq!(diff_lines(&lines(&c), &lines(&o))[2001], rep("o2001", ""));
    }

    #[test]
    fn falls_back_to_index_alignment_for_very_large_inputs() {
        let o: Vec<String> = (0..2001).map(|i| format!("o{i}")).collect();
        let c: Vec<String> = (0..2001)
            .map(|i| if i == 5 { "x".into() } else { format!("o{i}") })
            .collect();
        let rows = diff_lines(&lines(&o), &lines(&c));
        assert_eq!(rows.len(), 2001);
        assert_eq!(rows[5], rep("o5", "x"));
        assert_eq!(rows[6], Row::Equal("o6".into()));
    }

    #[test]
    fn marks_a_one_for_one_swap_on_both_sides() {
        assert_eq!(
            changed("你能站起来吗", "你能站起來嗎"),
            (vec!["来吗".into()], vec!["來嗎".into()])
        );
    }

    #[test]
    fn marks_separate_runs_separately_when_lengths_match() {
        let (o, c) = changed("这是软件说明", "這是軟件說明");
        assert_eq!(c, ["這", "軟", "說"]);
        assert_eq!(o, ["这", "软", "说"]);
    }

    #[test]
    fn finds_kept_characters_when_a_conversion_adds_one() {
        let (o, c) = changed("发生了什么 - 我刚刚", "發生了什麼事 - 我剛剛");
        assert_eq!(o, ["发", "么", "刚刚"]);
        assert_eq!(c, ["發", "麼事", "剛剛"]);
    }

    #[test]
    fn marks_the_whole_middle_when_lengths_differ_past_the_lcs_limit() {
        let (_, c) = changed(
            &format!("甲{}", "字".repeat(400)),
            &format!("乙丙{}", "文".repeat(400)),
        );
        assert_eq!(c, [format!("乙丙{}", "文".repeat(400))]);
    }

    #[test]
    fn marks_a_long_pure_insertion_or_deletion_on_one_side_only() {
        let inserted = "字".repeat(400);
        let long = format!("頭{inserted}尾");
        assert_eq!(changed("頭尾", &long), (vec![], vec![inserted.clone()]));
        assert_eq!(changed(&long, "頭尾"), (vec![inserted], vec![]));
    }

    #[test]
    fn marks_the_whole_middle_when_nothing_in_it_is_shared() {
        let (o, c) = changed("打开内存设置", "打開記憶體設定");
        assert_eq!(o, ["开内存设置"]);
        assert_eq!(c, ["開記憶體設定"]);
    }

    #[test]
    fn returns_no_changes_for_identical_lines() {
        assert_eq!(changed("00:00:01,000", "00:00:01,000"), (vec![], vec![]));
    }

    #[test]
    fn keeps_astral_chars_whole() {
        let (o, c) = inline_diff("𠀋发", "𠀋發");
        assert_eq!(c, vec![4..7], "𠀋 is 4 bytes and unchanged");
        assert_eq!(o, vec![4..7]);
    }
}
