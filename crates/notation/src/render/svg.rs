// SPDX-License-Identifier: GPL-3.0-or-later
//! SVG output, one document per page.

use std::fmt::Write;

use super::{Fonts, Seg, circle, knockout_box};
use crate::layout::{Item, Page};

/// Draw a page as a standalone SVG document, sized in points.
pub fn page_to_svg(page: &Page, fonts: &Fonts<'_>) -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}pt" height="{h:.2}pt" viewBox="0 0 {w:.2} {h:.2}">"#,
        w = page.width,
        h = page.height
    );
    let _ = write!(s, r##"<rect width="100%" height="100%" fill="#fff"/><g fill="#000" stroke="#000">"##);
    for item in &page.items {
        match item {
            Item::Line { from, to, width, dashed } => {
                let dash = if *dashed {
                    format!(r#" stroke-dasharray="{:.2} {:.2}""#, width * 8.0, width * 6.0)
                } else {
                    String::new()
                };
                let _ = write!(
                    s,
                    r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke-width="{:.2}"{dash}/>"#,
                    from.0, from.1, to.0, to.1, width
                );
            }
            Item::Glyph { at, glyph, size } => fill(&mut s, &fonts.glyph(*glyph, *at, *size)),
            Item::Text { at, text, size, anchor, bold, knockout } => {
                let (segs, left) = fonts.text(text, *at, *size, *anchor);
                if *knockout {
                    let (x, y, w, h) = knockout_box(left, fonts.text_width(text, *size), at.1, *size);
                    let _ = write!(
                        s,
                        r##"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" fill="#fff" stroke="none"/>"##
                    );
                }
                if *bold {
                    let _ = write!(s, r#"<path stroke-width="{:.2}" d=""#, size * 0.035);
                    path_data(&mut s, &segs);
                    s.push_str("\"/>");
                } else {
                    fill(&mut s, &segs);
                }
            }
            Item::Curve { from, c1, c2, to, width } => {
                let _ = write!(
                    s,
                    r#"<path fill="none" stroke-width="{width:.2}" d="M{:.2} {:.2}C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}"/>"#,
                    from.0, from.1, c1.0, c1.1, c2.0, c2.1, to.0, to.1
                );
            }
            Item::Polygon { points } => {
                s.push_str(r#"<polygon stroke="none" points=""#);
                for (x, y) in points {
                    let _ = write!(s, "{x:.2},{y:.2} ");
                }
                s.push_str("\"/>");
            }
            Item::Circle { centre, radius, fill: filled } => {
                if *filled {
                    fill(&mut s, &circle(*centre, *radius));
                } else {
                    let _ = write!(
                        s,
                        r#"<circle cx="{:.2}" cy="{:.2}" r="{radius:.2}" fill="none" stroke-width="{:.2}"/>"#,
                        centre.0,
                        centre.1,
                        radius * 0.3
                    );
                }
            }
        }
    }
    s.push_str("</g></svg>\n");
    s
}

fn fill(s: &mut String, segs: &[Seg]) {
    if segs.is_empty() {
        return;
    }
    s.push_str(r#"<path stroke="none" d=""#);
    path_data(s, segs);
    s.push_str("\"/>");
}

fn path_data(s: &mut String, segs: &[Seg]) {
    for seg in segs {
        let _ = match seg {
            Seg::Move(x, y) => write!(s, "M{x:.2} {y:.2}"),
            Seg::Line(x, y) => write!(s, "L{x:.2} {y:.2}"),
            Seg::Cubic(a, b, p) => write!(
                s,
                "C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}",
                a.0, a.1, b.0, b.1, p.0, p.1
            ),
            Seg::Close => write!(s, "Z"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{LayoutOptions, layout};
    use crate::render::tests::{MUSIC, TEXT};

    #[test]
    fn pages_become_svg() {
        let fonts = Fonts::new(MUSIC, TEXT).expect("fonts");
        let pages = layout(&crate::layout::tests::tab_score(4), &LayoutOptions::default());
        let svg = page_to_svg(&pages[0], &fonts);
        assert!(svg.starts_with("<svg"));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(svg.matches("<path").count() > 50);
        assert!(roxmltree::Document::parse(&svg).is_ok(), "well-formed XML");
    }
}
