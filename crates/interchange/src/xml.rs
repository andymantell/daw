// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing XML, indented, with attributes in the order given.

pub struct Xml {
    out: String,
    open: Vec<&'static str>,
}

/// An element's attributes: name and already formatted value. Empty
/// values are left out.
pub type Attrs<'a> = &'a [(&'a str, String)];

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\n' => o.push_str("&#10;"),
            '\r' => o.push_str("&#13;"),
            '\t' => o.push_str("&#9;"),
            // Not allowed in XML 1.0 at all.
            c if (c as u32) < 0x20 => {}
            c => o.push(c),
        }
    }
    o
}

impl Xml {
    pub fn new() -> Xml {
        Xml {
            out: String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n"),
            open: Vec::new(),
        }
    }

    fn start(&mut self, name: &str, attrs: Attrs) {
        for _ in 0..self.open.len() {
            self.out.push_str("  ");
        }
        self.out.push('<');
        self.out.push_str(name);
        for (k, v) in attrs {
            if v.is_empty() {
                continue;
            }
            self.out.push(' ');
            self.out.push_str(k);
            self.out.push_str("=\"");
            self.out.push_str(&esc(v));
            self.out.push('"');
        }
    }

    /// Opens an element, to be closed with [`Xml::close`].
    pub fn open(&mut self, name: &'static str, attrs: Attrs) {
        self.start(name, attrs);
        self.out.push_str(">\n");
        self.open.push(name);
    }

    /// An element with nothing in it.
    pub fn empty(&mut self, name: &str, attrs: Attrs) {
        self.start(name, attrs);
        self.out.push_str("/>\n");
    }

    /// An element holding only text.
    pub fn text(&mut self, name: &str, text: &str) {
        self.start(name, &[]);
        self.out.push('>');
        self.out.push_str(&esc(text));
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push_str(">\n");
    }

    pub fn close(&mut self) {
        let name = self.open.pop().expect("an element is open");
        for _ in 0..self.open.len() {
            self.out.push_str("  ");
        }
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push_str(">\n");
    }

    pub fn finish(self) -> String {
        assert!(self.open.is_empty(), "elements left open: {:?}", self.open);
        self.out
    }
}

/// `"true"` or `"false"`.
pub fn b(v: bool) -> String {
    if v { "true" } else { "false" }.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_nested_elements_and_escapes_text() {
        let mut x = Xml::new();
        x.open(
            "A",
            &[("name", "R&B \"mix\"".into()), ("skip", String::new())],
        );
        x.empty("B", &[("v", "1".into())]);
        x.text("C", "a<b\nc");
        x.close();
        let s = x.finish();
        assert!(s.contains("<A name=\"R&amp;B &quot;mix&quot;\">\n  <B v=\"1\"/>\n"));
        assert!(s.contains("<C>a&lt;b&#10;c</C>"));
        let doc = roxmltree::Document::parse(&s).unwrap();
        let a = doc.root_element();
        assert_eq!(a.attribute("name"), Some("R&B \"mix\""));
        assert_eq!(a.attribute("skip"), None);
    }
}
