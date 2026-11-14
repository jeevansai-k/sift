//! Squarified treemap layout (Bruls, Huijsen & van Wijk, 2000),
//! re-implemented from the paper.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

fn worst(row: &[f64], side: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    if sum <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let max = row.iter().cloned().fold(f64::MIN, f64::max);
    let min = row.iter().cloned().fold(f64::MAX, f64::min);
    let s2 = side * side;
    let sum2 = sum * sum;
    (s2 * max / sum2).max(sum2 / (s2 * min))
}

/// Lay out `sizes` (any order; zero/negative ignored) inside `bounds`.
/// Returns one rect per input, in input order (zero-size inputs get an
/// empty rect).
pub fn squarify(sizes: &[f64], bounds: Rect) -> Vec<Rect> {
    let mut out = vec![Rect { x: bounds.x, y: bounds.y, w: 0.0, h: 0.0 }; sizes.len()];
    let total: f64 = sizes.iter().filter(|s| **s > 0.0).sum();
    if total <= 0.0 || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return out;
    }
    let mut order: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i] > 0.0).collect();
    order.sort_by(|&a, &b| sizes[b].total_cmp(&sizes[a]));
    let scale = bounds.w * bounds.h / total;
    let areas: Vec<f64> = order.iter().map(|&i| sizes[i] * scale).collect();

    let mut free = bounds;
    let mut i = 0;
    while i < areas.len() {
        let side = free.w.min(free.h);
        let mut j = i + 1;
        while j < areas.len() && worst(&areas[i..=j], side) <= worst(&areas[i..j], side) {
            j += 1;
        }
        let row = &areas[i..j];
        let row_sum: f64 = row.iter().sum();
        if free.w >= free.h {
            // Column on the left.
            let col_w = if free.h > 0.0 { row_sum / free.h } else { 0.0 };
            let mut y = free.y;
            for (k, a) in row.iter().enumerate() {
                let h = if col_w > 0.0 { a / col_w } else { 0.0 };
                out[order[i + k]] = Rect { x: free.x, y, w: col_w, h };
                y += h;
            }
            free = Rect { x: free.x + col_w, y: free.y, w: (free.w - col_w).max(0.0), h: free.h };
        } else {
            // Row on top.
            let row_h = if free.w > 0.0 { row_sum / free.w } else { 0.0 };
            let mut x = free.x;
            for (k, a) in row.iter().enumerate() {
                let w = if row_h > 0.0 { a / row_h } else { 0.0 };
                out[order[i + k]] = Rect { x, y: free.y, w, h: row_h };
                x += w;
            }
            free = Rect { x: free.x, y: free.y + row_h, w: free.w, h: (free.h - row_h).max(0.0) };
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(sizes: &[f64], b: Rect) {
        let rects = squarify(sizes, b);
        let total: f64 = sizes.iter().filter(|s| **s > 0.0).sum();
        let mut area_sum = 0.0;
        for (s, r) in sizes.iter().zip(&rects) {
            let area = r.w * r.h;
            area_sum += area;
            if *s > 0.0 {
                let expect = s / total * b.w * b.h;
                assert!((area - expect).abs() < 1e-6 * b.w * b.h, "area proportional");
                assert!(r.x >= b.x - 1e-9 && r.y >= b.y - 1e-9);
                assert!(r.x + r.w <= b.x + b.w + 1e-6 && r.y + r.h <= b.y + b.h + 1e-6, "inside bounds");
            } else {
                assert_eq!(area, 0.0);
            }
        }
        assert!((area_sum - b.w * b.h).abs() < 1e-6 * b.w * b.h);
        // No overlaps between non-empty rects.
        for (i, a) in rects.iter().enumerate() {
            for c in rects.iter().skip(i + 1) {
                let ox = (a.x + a.w).min(c.x + c.w) - a.x.max(c.x);
                let oy = (a.y + a.h).min(c.y + c.h) - a.y.max(c.y);
                assert!(ox <= 1e-6 || oy <= 1e-6, "overlap");
            }
        }
    }

    #[test]
    fn paper_example_and_invariants() {
        let b = Rect { x: 0.0, y: 0.0, w: 6.0, h: 4.0 };
        check(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], b);
        check(&[1.0, 0.0, 5.0, 2.5], Rect { x: 10.0, y: 20.0, w: 300.0, h: 100.0 });
        check(&[42.0], Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });
        check(&(1..60).map(|i| (i * i) as f64).collect::<Vec<_>>(), Rect { x: 0.0, y: 0.0, w: 800.0, h: 500.0 });
        assert!(squarify(&[], b).is_empty());
    }

    #[test]
    fn rects_are_reasonably_square() {
        let rects = squarify(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], Rect { x: 0.0, y: 0.0, w: 6.0, h: 4.0 });
        let worst = rects.iter().map(|r| (r.w / r.h).max(r.h / r.w)).fold(0.0, f64::max);
        assert!(worst < 3.0);
    }
}
