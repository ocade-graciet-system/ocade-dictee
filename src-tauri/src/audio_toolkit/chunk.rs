use std::ops::Range;

/// Découpe [0, total_len) en tronçons de `chunk_len` échantillons avançant de
/// `chunk_len - overlap`. Le dernier tronçon est tronqué à `total_len`.
pub fn chunk_ranges(total_len: usize, chunk_len: usize, overlap: usize) -> Vec<Range<usize>> {
    assert!(
        chunk_len > 0 && overlap < chunk_len,
        "overlap doit être < chunk_len"
    );
    if total_len == 0 {
        return Vec::new();
    }
    if total_len <= chunk_len {
        return vec![0..total_len];
    }
    let step = chunk_len - overlap;
    let mut out = Vec::new();
    let mut start = 0;
    while start < total_len {
        let end = (start + chunk_len).min(total_len);
        out.push(start..end);
        if end == total_len {
            break;
        }
        start += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::chunk_ranges;

    #[test]
    fn shorter_than_chunk_is_single_range() {
        assert_eq!(chunk_ranges(1000, 16_000, 1_600), vec![0..1000]);
    }

    #[test]
    fn exact_multiple_without_overlap() {
        // chunk 100, overlap 0 sur 300 → 3 tronçons jointifs
        assert_eq!(chunk_ranges(300, 100, 0), vec![0..100, 100..200, 200..300]);
    }

    #[test]
    fn overlap_advances_by_step_and_covers_tail() {
        // chunk 100, overlap 20 → pas de 80 ; couvre jusqu'à la fin sans dépasser
        let r = chunk_ranges(250, 100, 20);
        assert_eq!(r.first().unwrap().start, 0);
        assert_eq!(r.last().unwrap().end, 250);
        // aucun range ne dépasse la longueur totale
        assert!(r.iter().all(|x| x.end <= 250));
        // chaque tronçon (sauf le dernier) fait chunk_len
        for w in &r[..r.len() - 1] {
            assert_eq!(w.end - w.start, 100);
        }
    }

    #[test]
    fn empty_input_yields_no_ranges() {
        assert_eq!(
            chunk_ranges(0, 100, 20),
            Vec::<std::ops::Range<usize>>::new()
        );
    }
}
