//! box drawing, block and powerline glyphs drawn as shapes that fill the cell exactly, so lines
//! connect across cells at any line height, which font glyphs sized for the font don't

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, Window, fill, point, px, size};

/// one piece of a glyph, in window coordinates
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Shape {
    /// filled rect, the alpha scales the glyph color, for shades
    Rect(Bounds<Pixels>, f32),
    /// filled closed polygon
    Fill(Vec<Point<Pixels>>),
    /// open polyline with a line width
    Stroke(Vec<Point<Pixels>>, Pixels),
}

// arm weights of U+2500-257F as up, right, down, left: 0 none, 1 light, 2 heavy, 3 double.
// dashes, arcs and diagonals are drawn by their own code
const ARMS: [&str; 128] = [
    "0101", "0202", "1010", "2020", "0101", "0202", "1010", "2020", // 2500
    "0101", "0202", "1010", "2020", "0110", "0210", "0120", "0220", // 2508
    "0011", "0012", "0021", "0022", "1100", "1200", "2100", "2200", // 2510
    "1001", "1002", "2001", "2002", "1110", "1210", "2110", "1120", // 2518
    "2120", "2210", "1220", "2220", "1011", "1012", "2011", "1021", // 2520
    "2021", "2012", "1022", "2022", "0111", "0112", "0211", "0212", // 2528
    "0121", "0122", "0221", "0222", "1101", "1102", "1201", "1202", // 2530
    "2101", "2102", "2201", "2202", "1111", "1112", "1211", "1212", // 2538
    "2111", "1121", "2121", "2112", "2211", "1122", "1221", "2212", // 2540
    "1222", "2122", "2221", "2222", "0101", "0202", "1010", "2020", // 2548
    "0303", "3030", "0310", "0130", "0330", "0013", "0031", "0033", // 2550
    "1300", "3100", "3300", "1003", "3001", "3003", "1310", "3130", // 2558
    "3330", "1013", "3031", "3033", "0313", "0131", "0333", "1303", // 2560
    "3101", "3303", "1313", "3131", "3333", "0110", "0011", "1001", // 2568
    "1100", "0000", "0000", "0000", "0001", "1000", "0100", "0010", // 2570
    "0002", "2000", "0200", "0020", "0201", "1020", "0102", "2010", // 2578
];

const UP: usize = 0;
const RIGHT: usize = 1;
const DOWN: usize = 2;
const LEFT: usize = 3;
const DOUBLE: u8 = 3;

/// true for chars drawn here instead of with the font
pub(super) fn supports(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{259F}' | '\u{E0B0}'..='\u{E0BF}')
}

/// shapes of `c` filling `cell`, empty for chars that are not supported
pub(super) fn shapes(c: char, cell: Bounds<Pixels>) -> Vec<Shape> {
    let code = c as u32;
    match code {
        0x2504..=0x250B | 0x254C..=0x254F => dashes(code, cell),
        0x256D..=0x2570 => arc(code, cell),
        0x2571..=0x2573 => diagonals(code, cell),
        0x2500..=0x257F => {
            let arms = ARMS[(code - 0x2500) as usize].as_bytes();
            lines([arms[0], arms[1], arms[2], arms[3]].map(|b| b - b'0'), cell)
        }
        0x2580..=0x259F => blocks(code, cell),
        0xE0B0..=0xE0BF => powerline(code, cell),
        _ => Vec::new(),
    }
}

/// paint `c` into `cell`
pub(super) fn paint(c: char, cell: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    for shape in shapes(c, cell) {
        let (mut path, points, closed) = match shape {
            Shape::Rect(bounds, alpha) => {
                window.paint_quad(fill(bounds, color.opacity(alpha)));
                continue;
            }
            Shape::Fill(points) => (PathBuilder::fill(), points, true),
            Shape::Stroke(points, width) => (PathBuilder::stroke(width), points, false),
        };
        path.add_polygon(&points, closed);
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

// light line width, heavy lines are twice as wide, double lines are two light ones
fn light(cell: Bounds<Pixels>) -> f32 {
    (f32::from(cell.size.width) / 8.).round().max(1.)
}

// edges are rounded to whole pixels, so neighbor cells share them and lines connect
fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
    let (x0, y0) = (x0.round(), y0.round());
    let (x1, y1) = (x1.round().max(x0 + 1.), y1.round().max(y0 + 1.));
    Shape::Rect(
        Bounds::new(point(px(x0), px(y0)), size(px(x1 - x0), px(y1 - y0))),
        1.,
    )
}

fn edges(cell: Bounds<Pixels>) -> (f32, f32, f32, f32) {
    (
        cell.origin.x.into(),
        cell.origin.y.into(),
        cell.right().into(),
        cell.bottom().into(),
    )
}

// straight lines from the center to the edges named by `arms` (up, right, down, left)
fn lines(arms: [u8; 4], cell: Bounds<Pixels>) -> Vec<Shape> {
    let t = light(cell);
    // centers of the two lines of a double arm sit this far from the middle
    let gap = t;
    let (x0, y0, x1, y1) = edges(cell);
    let (cx, cy) = ((x0 + x1) / 2., (y0 + y1) / 2.);
    let width = |weight: u8| if weight == 2 { 2. * t } else { t };
    // how far past the middle an arm reaches to meet a crossing arm of this weight
    let reach = |weight: u8| match weight {
        0 => 0.,
        DOUBLE => gap + t / 2.,
        weight => width(weight) / 2.,
    };

    let mut shapes = Vec::new();
    for arm in [UP, RIGHT, DOWN, LEFT] {
        let weight = arms[arm];
        if weight == 0 {
            continue;
        }
        let horizontal = arm == RIGHT || arm == LEFT;
        // crossing arms, `before` is on the negative offset side
        let (before, after) = if horizontal {
            (arms[UP], arms[DOWN])
        } else {
            (arms[LEFT], arms[RIGHT])
        };
        let opposite = arms[(arm + 2) % 4];
        let offsets: &[f32] = if weight == DOUBLE {
            &[-gap, gap]
        } else {
            &[0.]
        };
        for &offset in offsets {
            // where the line starts, as distance from the middle toward its edge
            let start = if offset == 0. {
                if before == DOUBLE && after == DOUBLE && opposite == 0 {
                    // ends at the near line of a double line passing by, like ╟
                    gap - t / 2.
                } else if before == DOUBLE && after == DOUBLE {
                    // crosses a double line, like ╫
                    0.
                } else {
                    -reach(before).max(reach(after))
                }
            } else {
                let (side, other) = if offset < 0. {
                    (before, after)
                } else {
                    (after, before)
                };
                match side {
                    // inner corner with a double arm on the same side, like the inside of ╔
                    DOUBLE => gap - t / 2.,
                    0 => -reach(other),
                    side => -width(side) / 2.,
                }
            };
            let half = width(weight) / 2.;
            shapes.push(match arm {
                UP => rect(cx + offset - half, y0, cx + offset + half, cy - start),
                DOWN => rect(cx + offset - half, cy + start, cx + offset + half, y1),
                LEFT => rect(x0, cy + offset - half, cx - start, cy + offset + half),
                _ => rect(cx + start, cy + offset - half, x1, cy + offset + half),
            });
        }
    }
    shapes
}

fn dashes(code: u32, cell: Bounds<Pixels>) -> Vec<Shape> {
    let (count, vertical, heavy) = match code {
        0x2504..=0x250B => {
            let i = code - 0x2504;
            (if i < 4 { 3 } else { 4 }, i % 4 >= 2, i % 2 == 1)
        }
        _ => {
            let i = code - 0x254C;
            (2, i >= 2, i % 2 == 1)
        }
    };
    let t = light(cell) * if heavy { 2. } else { 1. };
    let (x0, y0, x1, y1) = edges(cell);
    let (cx, cy) = ((x0 + x1) / 2., (y0 + y1) / 2.);
    let (from, to) = if vertical { (y0, y1) } else { (x0, x1) };
    let part = (to - from) / count as f32;
    (0..count)
        .map(|i| {
            // a quarter part of gap on both ends, so gaps between cells match gaps inside
            let a = from + i as f32 * part + part / 4.;
            let b = a + part / 2.;
            if vertical {
                rect(cx - t / 2., a, cx + t / 2., b)
            } else {
                rect(a, cy - t / 2., b, cy + t / 2.)
            }
        })
        .collect()
}

// round corners ╭╮╯╰: a quarter circle between straight ends at the cell edges
fn arc(code: u32, cell: Bounds<Pixels>) -> Vec<Shape> {
    let (x0, y0, x1, y1) = edges(cell);
    let (cx, cy) = ((x0 + x1) / 2., (y0 + y1) / 2.);
    // direction of the vertical and the horizontal arm
    let (dy, dx) = match code {
        0x256D => (1., 1.),
        0x256E => (1., -1.),
        0x256F => (-1., -1.),
        _ => (-1., 1.),
    };
    let r = (x1 - x0).min(y1 - y0) / 2.;
    let center = (cx + dx * r, cy + dy * r);
    let mut points = vec![point(px(cx), px(if dy > 0. { y1 } else { y0 }))];
    for step in 0..=8 {
        let angle = step as f32 / 8. * std::f32::consts::FRAC_PI_2;
        points.push(point(
            px(center.0 - dx * r * angle.cos()),
            px(center.1 - dy * r * angle.sin()),
        ));
    }
    points.push(point(px(if dx > 0. { x1 } else { x0 }), px(cy)));
    vec![Shape::Stroke(points, px(light(cell)))]
}

fn diagonals(code: u32, cell: Bounds<Pixels>) -> Vec<Shape> {
    let (x0, y0, x1, y1) = edges(cell);
    let width = px(light(cell));
    let rising = Shape::Stroke(vec![point(px(x1), px(y0)), point(px(x0), px(y1))], width);
    let falling = Shape::Stroke(vec![point(px(x0), px(y0)), point(px(x1), px(y1))], width);
    match code {
        0x2571 => vec![rising],
        0x2572 => vec![falling],
        _ => vec![rising, falling],
    }
}

fn blocks(code: u32, cell: Bounds<Pixels>) -> Vec<Shape> {
    let (x0, y0, x1, y1) = edges(cell);
    let (w, h) = (x1 - x0, y1 - y0);
    // part of the cell in eighths: left, top, right, bottom
    let eighths = |l: f32, t: f32, r: f32, b: f32| {
        rect(
            x0 + w * l / 8.,
            y0 + h * t / 8.,
            x0 + w * r / 8.,
            y0 + h * b / 8.,
        )
    };
    let shade = |alpha: f32| match rect(x0, y0, x1, y1) {
        Shape::Rect(bounds, _) => Shape::Rect(bounds, alpha),
        shape => shape,
    };
    let n = (code - 0x2580) as f32;
    match code {
        0x2580 => vec![eighths(0., 0., 8., 4.)],
        // lower one eighth to full block
        0x2581..=0x2588 => vec![eighths(0., 8. - n, 8., 8.)],
        // left seven eighths to left one eighth
        0x2589..=0x258F => vec![eighths(0., 0., 16. - n, 8.)],
        0x2590 => vec![eighths(4., 0., 8., 8.)],
        0x2591..=0x2593 => vec![shade((n - 16.) / 4.)],
        0x2594 => vec![eighths(0., 0., 8., 1.)],
        0x2595 => vec![eighths(7., 0., 8., 8.)],
        _ => {
            // quadrants as bits: upper left, upper right, lower left, lower right
            let bits = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14][(code - 0x2596) as usize];
            [(0., 0.), (4., 0.), (0., 4.), (4., 4.)]
                .into_iter()
                .enumerate()
                .filter(|(i, _)| bits & (1 << i) != 0)
                .map(|(_, (l, t))| eighths(l, t, l + 4., t + 4.))
                .collect()
        }
    }
}

fn powerline(code: u32, cell: Bounds<Pixels>) -> Vec<Shape> {
    let (x0, y0, x1, y1) = edges(cell);
    let (w, h, cy) = (x1 - x0, y1 - y0, (y0 + y1) / 2.);
    let p = |x: f32, y: f32| point(px(x), px(y));
    // half ellipse from the top to the bottom of the cell, bulging away from `flat_x`
    let half_circle = |flat_x: f32, direction: f32| -> Vec<Point<Pixels>> {
        (0..=16)
            .map(|step| {
                let angle = (step as f32 / 16. - 0.5) * std::f32::consts::PI;
                p(
                    flat_x + direction * w * angle.cos(),
                    cy + h / 2. * angle.sin(),
                )
            })
            .collect()
    };
    let width = px(light(cell));
    let shape = match code {
        0xE0B0 => Shape::Fill(vec![p(x0, y0), p(x1, cy), p(x0, y1)]),
        0xE0B1 => Shape::Stroke(vec![p(x0, y0), p(x1, cy), p(x0, y1)], width),
        0xE0B2 => Shape::Fill(vec![p(x1, y0), p(x0, cy), p(x1, y1)]),
        0xE0B3 => Shape::Stroke(vec![p(x1, y0), p(x0, cy), p(x1, y1)], width),
        0xE0B4 => Shape::Fill(half_circle(x0, 1.)),
        0xE0B5 => Shape::Stroke(half_circle(x0, 1.), width),
        0xE0B6 => Shape::Fill(half_circle(x1, -1.)),
        0xE0B7 => Shape::Stroke(half_circle(x1, -1.), width),
        0xE0B8 => Shape::Fill(vec![p(x0, y0), p(x1, y1), p(x0, y1)]),
        0xE0BA => Shape::Fill(vec![p(x1, y0), p(x1, y1), p(x0, y1)]),
        0xE0BC => Shape::Fill(vec![p(x0, y0), p(x1, y0), p(x0, y1)]),
        0xE0BE => Shape::Fill(vec![p(x0, y0), p(x1, y0), p(x1, y1)]),
        0xE0B9 | 0xE0BF => Shape::Stroke(vec![p(x0, y0), p(x1, y1)], width),
        _ => Shape::Stroke(vec![p(x1, y0), p(x0, y1)], width),
    };
    vec![shape]
}

#[cfg(test)]
mod tests {
    use super::*;

    // odd sizes, so rounding has to line things up
    fn cell() -> Bounds<Pixels> {
        Bounds::new(point(px(13.), px(7.)), size(px(12.4), px(27.)))
    }

    fn rects(c: char) -> Vec<Bounds<Pixels>> {
        shapes(c, cell())
            .into_iter()
            .map(|shape| match shape {
                Shape::Rect(bounds, _) => bounds,
                shape => panic!("{c:?} drew {shape:?}"),
            })
            .collect()
    }

    // pixel edges of the cell after rounding, what neighbor cells share
    fn cell_edges() -> (Pixels, Pixels, Pixels, Pixels) {
        let cell = cell();
        (
            cell.origin.x.round(),
            cell.origin.y.round(),
            cell.right().round(),
            cell.bottom().round(),
        )
    }

    fn covers(rects: &[Bounds<Pixels>], p: Point<Pixels>) -> bool {
        rects.iter().any(|r| r.contains(&p))
    }

    #[test]
    fn supported_ranges() {
        for c in [
            '\u{2500}', '\u{257f}', '\u{2580}', '\u{259f}', '\u{e0b0}', '\u{e0bf}',
        ] {
            assert!(supports(c), "{c:?}");
        }
        for c in ['\u{24ff}', '\u{25a0}', '\u{e0af}', '\u{e0c0}', 'a'] {
            assert!(!supports(c), "{c:?}");
        }
    }

    #[test]
    fn every_supported_char_draws_inside_its_cell() {
        let cell = cell();
        let chars = ('\u{2500}'..='\u{259f}').chain('\u{e0b0}'..='\u{e0bf}');
        for c in chars {
            let shapes = shapes(c, cell);
            assert!(!shapes.is_empty(), "{c:?} draws nothing");
            for shape in shapes {
                let points = match shape {
                    Shape::Rect(bounds, alpha) => {
                        assert!(alpha > 0. && alpha <= 1., "{c:?} alpha {alpha}");
                        vec![bounds.origin, bounds.bottom_right()]
                    }
                    Shape::Fill(points) | Shape::Stroke(points, _) => points,
                };
                for p in points {
                    // rounding may move an edge by half a pixel
                    assert!(
                        p.x >= cell.origin.x - px(0.5)
                            && p.x <= cell.right() + px(0.5)
                            && p.y >= cell.origin.y - px(0.5)
                            && p.y <= cell.bottom() + px(0.5),
                        "{c:?} draws at {p:?} outside {cell:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn straight_lines_reach_the_rounded_cell_edges() {
        let (left, top, right, bottom) = cell_edges();
        let line = rects('─');
        assert_eq!(line.len(), 2);
        assert!(line.iter().any(|r| r.origin.x == left));
        assert!(line.iter().any(|r| r.right() == right));
        // both halves sit on the same row, so they join
        assert_eq!(line[0].origin.y, line[1].origin.y);
        let line = rects('│');
        assert!(line.iter().any(|r| r.origin.y == top));
        assert!(line.iter().any(|r| r.bottom() == bottom));
        // heavy is twice as thick as light
        assert_eq!(rects('━')[0].size.height, rects('─')[0].size.height * 2.);
    }

    #[test]
    fn corners_and_crosses_have_no_hole_in_the_middle() {
        let cell = cell();
        let middle = cell.center();
        for c in [
            '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '┏', '╋', '┍', '╀',
        ] {
            assert!(covers(&rects(c), middle), "{c:?} has a hole in the middle");
        }
        // double lines leave the middle empty and keep the outside of the corner closed
        let corner = rects('╔');
        assert!(!covers(&corner, middle));
        assert_eq!(corner.len(), 4);
        // ╫ crosses the double line, ╟ stops at it
        assert!(covers(&rects('╫'), middle));
        assert!(!covers(&rects('╟'), middle));
    }

    #[test]
    fn dashes_have_gaps() {
        assert_eq!(rects('┄').len(), 3);
        assert_eq!(rects('┊').len(), 4);
        assert_eq!(rects('╌').len(), 2);
        let (left, _, right, _) = cell_edges();
        let dashes = rects('┈');
        let drawn: Pixels = dashes.iter().map(|r| r.size.width).sum();
        assert!(drawn < right - left);
    }

    #[test]
    fn blocks_fill_their_part_of_the_cell() {
        let (left, top, right, bottom) = cell_edges();
        let full = rects('█');
        assert_eq!(
            full,
            [Bounds::from_corners(point(left, top), point(right, bottom))]
        );
        let upper = rects('▀')[0];
        assert_eq!((upper.origin.y, upper.right()), (top, right));
        assert!(upper.bottom() < bottom);
        let left_half = rects('▌')[0];
        assert_eq!(left_half.origin.x, left);
        assert!(left_half.right() < right);
        assert_eq!(rects('▚').len(), 2);
        assert_eq!(rects('▟').len(), 3);
        for (c, alpha) in [('░', 0.25), ('▒', 0.5), ('▓', 0.75)] {
            assert!(
                matches!(shapes(c, cell())[..], [Shape::Rect(_, a)] if a == alpha),
                "{c:?}"
            );
        }
    }

    #[test]
    fn powerline_triangles_fill_the_cell_height() {
        let cell = cell();
        let Shape::Fill(points) = &shapes('\u{e0b0}', cell)[0] else {
            panic!("solid arrow is not filled");
        };
        assert!(points.contains(&cell.origin));
        assert!(points.contains(&cell.bottom_left()));
        assert!(points.contains(&point(cell.right(), cell.center().y)));
        assert!(matches!(shapes('\u{e0b1}', cell)[0], Shape::Stroke(..)));
    }
}
