/// A rectangle in terminal cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

struct Area {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Squarified treemap over a `w x h` cell area. Returns one entry per input
/// weight, in input order: `None` for zero weights and for items too small
/// to get a whole cell. Rectangles never overlap and, whenever any weight is
/// positive, together cover the whole area exactly (dropped items' space goes
/// to their neighbors). A terminal cell is about twice as tall as it is wide,
/// so aspect ratios are computed on `w x (2h)`.
pub fn squarify(weights: &[u64], w: u16, h: u16) -> Vec<Option<CellRect>> {
    let mut out = vec![None; weights.len()];
    if w == 0 || h == 0 {
        return out;
    }
    let mut items: Vec<(usize, u64)> = weights
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, wt)| *wt > 0)
        .collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut rect = Area {
        x: 0,
        y: 0,
        w: w as u32,
        h: h as u32,
    };
    let mut rest: &[(usize, u64)] = &items;
    while !rest.is_empty() && rect.w > 0 && rect.h > 0 {
        let total: f64 = rest.iter().map(|i| i.1 as f64).sum();
        // Wide areas get a column on the left; tall areas get a row on top.
        let wide = rect.w as f64 >= rect.h as f64 * 2.0;
        let (primary_real, secondary_real) = if wide {
            (rect.w as f64, rect.h as f64 * 2.0)
        } else {
            (rect.h as f64 * 2.0, rect.w as f64)
        };
        let (primary_cells, secondary_cells) = if wide {
            (rect.w, rect.h)
        } else {
            (rect.h, rect.w)
        };

        let worst = |k: usize| -> f64 {
            let row_sum: f64 = rest[..k].iter().map(|i| i.1 as f64).sum();
            let thickness = row_sum / total * primary_real;
            rest[..k]
                .iter()
                .map(|i| {
                    let len = i.1 as f64 / row_sum * secondary_real;
                    (thickness / len).max(len / thickness)
                })
                .fold(0.0, f64::max)
        };
        let mut k = 1;
        let mut best = worst(1);
        while k < rest.len() {
            let next = worst(k + 1);
            if next <= best {
                best = next;
                k += 1;
            } else {
                break;
            }
        }

        let row = &rest[..k];
        let row_sum: u64 = row.iter().map(|i| i.1).sum();
        let thick = if k == rest.len() {
            primary_cells
        } else {
            let t = (row_sum as f64 / total * primary_cells as f64).round() as u32;
            t.clamp(1, primary_cells)
        };

        let mut cum: u128 = 0;
        let mut prev_edge: u32 = 0;
        for &(idx, wt) in row {
            cum += wt as u128;
            let edge =
                ((cum * secondary_cells as u128 + row_sum as u128 / 2) / row_sum as u128) as u32;
            let span = edge - prev_edge;
            if span > 0 {
                let (x, y, cw, ch) = if wide {
                    (rect.x, rect.y + prev_edge, thick, span)
                } else {
                    (rect.x + prev_edge, rect.y, span, thick)
                };
                out[idx] = Some(CellRect {
                    x: x as u16,
                    y: y as u16,
                    w: cw as u16,
                    h: ch as u16,
                });
            }
            prev_edge = edge;
        }

        if wide {
            rect.x += thick;
            rect.w -= thick;
        } else {
            rect.y += thick;
            rect.h -= thick;
        }
        rest = &rest[k..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(r: &CellRect) -> u32 {
        r.w as u32 * r.h as u32
    }

    /// Paint every rect on a grid; panics on overlap or out-of-bounds.
    /// Returns how many cells are covered.
    fn covered_cells(rects: &[Option<CellRect>], w: u16, h: u16) -> usize {
        let mut grid = vec![false; w as usize * h as usize];
        for r in rects.iter().flatten() {
            assert!(r.w > 0 && r.h > 0, "empty rect {r:?}");
            assert!(r.x + r.w <= w && r.y + r.h <= h, "out of bounds {r:?}");
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    let i = y as usize * w as usize + x as usize;
                    assert!(!grid[i], "overlap at ({x},{y})");
                    grid[i] = true;
                }
            }
        }
        grid.iter().filter(|c| **c).count()
    }

    #[test]
    fn empty_and_zero_area() {
        assert!(squarify(&[], 10, 10).is_empty());
        assert_eq!(squarify(&[5], 0, 5), vec![None]);
        assert_eq!(squarify(&[5], 5, 0), vec![None]);
    }

    #[test]
    fn single_item_fills_area() {
        assert_eq!(
            squarify(&[7], 10, 5),
            vec![Some(CellRect {
                x: 0,
                y: 0,
                w: 10,
                h: 5
            })]
        );
    }

    #[test]
    fn rects_inside_bounds_and_disjoint() {
        let rects = squarify(&[60, 30, 20, 10, 5, 1], 40, 12);
        assert_eq!(rects.len(), 6);
        covered_cells(&rects, 40, 12);
    }

    #[test]
    fn covers_whole_area_when_all_visible() {
        let rects = squarify(&[4, 3, 2, 1], 20, 10);
        assert!(rects.iter().all(Option::is_some));
        assert_eq!(covered_cells(&rects, 20, 10), 200);
        let sum: u32 = rects.iter().flatten().map(area).sum();
        assert_eq!(sum, 200);
    }

    #[test]
    fn bigger_weight_gets_bigger_or_equal_area() {
        let rects = squarify(&[40, 20, 10], 30, 12);
        let a: Vec<u32> = rects.iter().map(|r| r.map_or(0, |r| area(&r))).collect();
        assert!(a[0] >= a[1] && a[1] >= a[2], "{a:?}");
    }

    #[test]
    fn zero_weight_is_none() {
        let rects = squarify(&[10, 0, 10], 20, 10);
        assert!(rects[0].is_some());
        assert!(rects[1].is_none());
        assert!(rects[2].is_some());
        assert_eq!(covered_cells(&rects, 20, 10), 200);
    }

    #[test]
    fn tiny_items_dropped_not_overlapping() {
        let rects = squarify(&[1000, 1, 1, 1], 8, 4);
        assert!(rects[0].is_some());
        covered_cells(&rects, 8, 4);
    }

    #[test]
    fn all_zero_weights_give_all_none() {
        assert_eq!(squarify(&[0, 0], 10, 10), vec![None, None]);
    }

    #[test]
    fn many_items_stay_disjoint_in_small_area() {
        let weights: Vec<u64> = (1..=50).rev().collect();
        let rects = squarify(&weights, 30, 8);
        assert_eq!(rects.len(), 50);
        covered_cells(&rects, 30, 8);
    }

    #[test]
    fn order_of_results_matches_input_order() {
        // The smallest weight comes first; it must still get the last slot's data.
        let rects = squarify(&[1, 100], 20, 10);
        let small = rects[0].map_or(0, |r| area(&r));
        let big = rects[1].map_or(0, |r| area(&r));
        assert!(big > small);
    }
}
