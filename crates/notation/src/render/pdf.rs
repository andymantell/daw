// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF output for printing: one PDF page per laid-out page, everything
//! drawn as vector outlines, so the file embeds no fonts.

use std::fmt::Write;

use super::{Fonts, Seg, circle, knockout_box};
use crate::layout::{Item, Page};

/// Draw pages as a PDF document.
pub fn pages_to_pdf(pages: &[Page], fonts: &Fonts<'_>) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    // 1: catalog, 2: page tree, then a page and its contents per page.
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", 3 + 2 * i))
        .collect();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages.len()
    ));
    for (i, page) in pages.iter().enumerate() {
        let content = page_content(page, fonts);
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Contents {} 0 R /Resources << >> >>",
            page.width,
            page.height,
            4 + 2 * i
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len() + 1
        ));
    }

    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let mut tail = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for o in offsets {
        let _ = writeln!(tail, "{o:010} 00000 n ");
    }
    let _ = write!(
        tail,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    );
    out.extend_from_slice(tail.as_bytes());
    out
}

fn page_content(page: &Page, fonts: &Fonts<'_>) -> String {
    let mut s = String::new();
    // Flip to y-down so page coordinates go straight in.
    let _ = writeln!(s, "1 0 0 -1 0 {:.2} cm 0 g 0 G 1 J", page.height);
    for item in &page.items {
        match item {
            Item::Line {
                from,
                to,
                width,
                dashed,
            } => {
                let dash = if *dashed {
                    format!("[{:.2} {:.2}] 0 d", width * 8.0, width * 6.0)
                } else {
                    "[] 0 d".into()
                };
                let _ = writeln!(
                    s,
                    "{width:.2} w {dash} {:.2} {:.2} m {:.2} {:.2} l S",
                    from.0, from.1, to.0, to.1
                );
            }
            Item::Glyph { at, glyph, size } => fill(&mut s, &fonts.glyph(*glyph, *at, *size)),
            Item::Text {
                at,
                text,
                size,
                anchor,
                bold,
                knockout,
            } => {
                let (segs, left) = fonts.text(text, *at, *size, *anchor);
                if *knockout {
                    let (x, y, w, h) =
                        knockout_box(left, fonts.text_width(text, *size), at.1, *size);
                    let _ = writeln!(s, "1 g {x:.2} {y:.2} {w:.2} {h:.2} re f 0 g");
                }
                if *bold {
                    path(&mut s, &segs);
                    let _ = writeln!(s, "{:.2} w [] 0 d B", size * 0.035);
                } else {
                    fill(&mut s, &segs);
                }
            }
            Item::Curve {
                from,
                c1,
                c2,
                to,
                width,
            } => {
                let _ = writeln!(
                    s,
                    "{width:.2} w [] 0 d {:.2} {:.2} m {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c S",
                    from.0, from.1, c1.0, c1.1, c2.0, c2.1, to.0, to.1
                );
            }
            Item::Polygon { points } => {
                for (i, (x, y)) in points.iter().enumerate() {
                    let _ = write!(s, "{x:.2} {y:.2} {} ", if i == 0 { "m" } else { "l" });
                }
                s.push_str("h f\n");
            }
            Item::Circle {
                centre,
                radius,
                fill: filled,
            } => {
                path(&mut s, &circle(*centre, *radius));
                if *filled {
                    s.push_str("f\n");
                } else {
                    let _ = writeln!(s, "{:.2} w [] 0 d S", radius * 0.3);
                }
            }
        }
    }
    s
}

fn path(s: &mut String, segs: &[Seg]) {
    for seg in segs {
        let _ = match seg {
            Seg::Move(x, y) => write!(s, "{x:.2} {y:.2} m "),
            Seg::Line(x, y) => write!(s, "{x:.2} {y:.2} l "),
            Seg::Cubic(a, b, p) => write!(
                s,
                "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c ",
                a.0, a.1, b.0, b.1, p.0, p.1
            ),
            Seg::Close => write!(s, "h "),
        };
    }
}

fn fill(s: &mut String, segs: &[Seg]) {
    if segs.is_empty() {
        return;
    }
    path(s, segs);
    s.push_str("f\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{LayoutOptions, layout};
    use crate::render::tests::{MUSIC, TEXT};

    #[test]
    fn pages_become_a_pdf_with_a_valid_cross_reference() {
        let fonts = Fonts::new(MUSIC, TEXT).expect("fonts");
        let pages = layout(
            &crate::layout::tests::tab_score(60),
            &LayoutOptions::default(),
        );
        let pdf = pages_to_pdf(&pages, &fonts);
        // The binary marker in the header is the only non-ASCII; blank it so
        // byte offsets match string offsets.
        let ascii: Vec<u8> = pdf
            .iter()
            .map(|&b| if b.is_ascii() { b } else { b'?' })
            .collect();
        let text = String::from_utf8(ascii).expect("ascii");
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.trim_end().ends_with("%%EOF"));
        assert!(text.contains(&format!("/Count {}", pages.len())));
        // Every cross-reference offset points at its object.
        let xref_at: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|t| t.lines().next())
            .and_then(|n| n.parse().ok())
            .expect("startxref");
        let table = &text[xref_at..];
        for (i, line) in table.lines().skip(3).take(2 * pages.len() + 2).enumerate() {
            let off: usize = line[..10].parse().expect("offset");
            assert!(
                text[off..].starts_with(&format!("{} 0 obj", i + 1)),
                "object {}",
                i + 1
            );
        }
    }
}
