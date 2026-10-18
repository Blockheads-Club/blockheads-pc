//! Frame-time statistics over the swap timestamps of a play run, split into segments by script `mark` commands.
use std::time::Instant;

/// Distribution of frame intervals in milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub frames: usize,
    pub mean: f64,
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
}

/// Percentile (nearest rank) of an already sorted, non-empty slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Statistics of the intervals between consecutive swaps (`None` with fewer than two swaps).
pub fn stats(swaps: &[Instant]) -> Option<Stats> {
    let mut d: Vec<f64> = swaps.windows(2).map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1000.0).collect();
    if d.is_empty() {
        return None;
    }
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    d.sort_by(f64::total_cmp);
    Some(Stats { frames: d.len(), mean, p50: percentile(&d, 50.0), p99: percentile(&d, 99.0), max: d[d.len() - 1] })
}

/// Report lines `label: ...` for every segment that starts at a mark (`(label, index of the first swap)`); the
/// segment before the first mark is not reported.
pub fn report(swaps: &[Instant], marks: &[(String, usize)]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, (label, start)) in marks.iter().enumerate() {
        let end = marks.get(i + 1).map_or(swaps.len(), |m| m.1).min(swaps.len());
        let from = (*start).min(end);
        out.push(match stats(&swaps[from..end]) {
            Some(s) => format!(
                "[frames] {label}: {} frames, mean {:.2} ms ({:.1} fps), p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
                s.frames,
                s.mean,
                1000.0 / s.mean,
                s.p50,
                s.p99,
                s.max
            ),
            None => format!("[frames] {label}: fewer than two frames"),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn series(gaps_ms: &[u64]) -> Vec<Instant> {
        let mut t = Instant::now();
        let mut v = vec![t];
        for g in gaps_ms {
            t += Duration::from_millis(*g);
            v.push(t);
        }
        v
    }

    #[test]
    fn percentiles_of_intervals() {
        assert_eq!(stats(&series(&[])), None);
        let mut gaps = vec![10u64; 99];
        gaps.push(100);
        let s = stats(&series(&gaps)).unwrap();
        assert_eq!(s.frames, 100);
        assert!((s.p50 - 10.0).abs() < 0.01);
        assert!((s.p99 - 10.0).abs() < 0.01);
        assert!((s.max - 100.0).abs() < 0.01);
        assert!((s.mean - 10.9).abs() < 0.01);
        gaps.push(100);
        assert!((stats(&series(&gaps)).unwrap().p99 - 100.0).abs() < 0.01);
    }

    #[test]
    fn segments_follow_marks() {
        let swaps = series(&[10, 10, 20, 20, 20]);
        let marks = vec![("a".to_string(), 0), ("b".to_string(), 3), ("late".to_string(), 99)];
        let r = report(&swaps, &marks);
        assert_eq!(r.len(), 3);
        assert!(r[0].starts_with("[frames] a: 2 frames, mean 10.00 ms"), "{}", r[0]);
        assert!(r[1].contains("b: 2 frames"), "{}", r[1]);
        assert!(r[2].contains("late: fewer than two frames"));
        assert!(report(&swaps, &[]).is_empty());
    }
}
