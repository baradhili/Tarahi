/// Split `text` into contiguous runs by which face of the measurement
/// chain covers each character: `(face_ix, text)` pairs where
/// `face_ix` indexes `[self, others…]` — a character an earlier face
/// lacks and a later one covers joins that later face's run, and a
/// character NOTHING covers stays on face 0 (the measuring face's
/// per-character fallback estimates it, exactly like the single-face
/// path). Shaping then happens per run with the face that paints it,
/// mirroring the raster's per-cluster fallback (usvg walks its
/// fontdb in load order — the registration order of
/// `dc::register_complex_script_faces`, which this chain must copy);
/// only cross-script run boundaries forgo cross-run kerning, which
/// does not exist across scripts anyway. The two-face form (PR #269
/// review) grew to the N-face chain with the CJK wave (#282):
/// Vazirmatn covers the Arabic script, Noto Sans SC the Han glyphs
/// the Latin faces lack.
pub(crate) fn split_runs(text: &str, covers: &[&dyn Fn(char) -> bool]) -> Vec<(usize, String)> {
    let mut runs: Vec<(usize, String)> = Vec::new();
    for c in text.chars() {
        let face_ix = covers.iter().position(|covers| covers(c)).unwrap_or(0);
        match runs.last_mut() {
            Some((last_ix, buf)) if *last_ix == face_ix => buf.push(c),
            _ => runs.push((face_ix, c.to_string())),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PR #269 review: runs split by which face covers each char —
    /// only chars an EARLIER face lacks AND a later one covers switch
    /// runs; chars nothing covers stay on face 0 (their advance falls
    /// through the missing-glyph chain either way).
    #[test]
    fn split_runs_follows_per_char_coverage() {
        let lower = |c: char| c.is_ascii_lowercase();
        let upper = |c: char| c.is_ascii_uppercase();
        let digit = |c: char| c.is_ascii_digit();
        let runs = split_runs("ab CD EF gh", &[&lower, &upper]);
        let flat: Vec<(usize, String)> = runs
            .into_iter()
            .map(|(o, t): (usize, String)| {
                (
                    o,
                    t.chars().filter(|c| !c.is_whitespace()).collect::<String>(),
                )
            })
            .filter(|(_, t)| !t.is_empty())
            .collect();
        assert_eq!(
            flat,
            vec![
                (0, "ab".into()),
                (1, "CD".into()),
                (1, "EF".into()),
                (0, "gh".into()),
            ],
            "contiguous covered spans join; the split follows coverage, not script"
        );
        // Chars NEITHER face covers stay on the primary run.
        let runs = split_runs("a中b", &[&lower, &upper]);
        assert_eq!(
            runs,
            vec![(0, "a中b".to_string())],
            "uncovered chars never switch runs"
        );
        // The N-face chain picks the FIRST covering face in order.
        let runs = split_runs("a1Ab2", &[&lower, &digit, &upper]);
        assert_eq!(
            runs,
            vec![
                (0, "a".into()),
                (1, "1".into()),
                (2, "A".into()),
                (0, "b".into()),
                (1, "2".into()),
            ],
            "chain order decides which fallback owns a run"
        );
    }
}
