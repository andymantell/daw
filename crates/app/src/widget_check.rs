// SPDX-License-Identifier: GPL-3.0-or-later
//! Every control in the app says how an assistant reaches what it does
//! (roadmap 2.11; docs/AI_ASSISTANTS.md).
//!
//! A button, checkbox, choice, slider or text field drawn with egui is
//! marked with a comment on its line or one of the three above it:
//!
//! ```text
//! // ai: view            it changes only what is shown (a window, a tab)
//! // ai: add-bus         a command's id, which it runs
//! // ai: set_parameter   an assistant tool that does the same
//! ```
//!
//! The test here reads the app's source and checks each name is a
//! command or a tool. Controls from before the rule are counted per file
//! in `widgets-unmarked.txt`, and a file may only have fewer: a new
//! control must be marked, and marking old ones lowers the count.

use std::collections::BTreeMap;
use std::path::Path;

/// The egui calls that draw a control a person changes things with.
const CONTROLS: &[&str] = &[
    ".button(",
    ".small_button(",
    ".checkbox(",
    ".selectable_label(",
    ".selectable_value(",
    ".radio_value(",
    ".radio(",
    ".toggle_value(",
    "Slider::new(",
    "DragValue::new(",
    "Button::new(",
    "IconButton::labelled(",
    "TextEdit::singleline(",
    "TextEdit::multiline(",
    ".text_edit_singleline(",
    ".text_edit_multiline(",
];

/// How far above a control its mark may be.
const MARK_REACH: usize = 3;

/// What a mark names, if `line` is one.
fn mark(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once("// ai:")?;
    rest.split_whitespace().next()
}

/// The source before a file's tests, which may draw what they like.
fn before_tests(src: &str) -> &str {
    let mut at = 0;
    while let Some(i) = src[at..].find("\n#[cfg(test)]\nmod ") {
        let start = at + i;
        let line = src[start + 1..].lines().nth(1).unwrap_or("");
        if line.trim_end().ends_with('{') {
            return &src[..start];
        }
        at = start + 1;
    }
    src
}

/// For one file: the controls without a mark, and the names marks give
/// (with their line numbers).
fn scan(src: &str) -> (usize, Vec<(usize, String)>) {
    let lines: Vec<&str> = before_tests(src).lines().collect();
    let mut unmarked = 0;
    let mut names = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(name) = mark(line) {
            names.push((i + 1, name.to_string()));
        }
        let code = line.split("//").next().unwrap_or("");
        let controls = CONTROLS.iter().filter(|c| code.contains(*c)).count();
        if controls == 0 {
            continue;
        }
        let marked = (i.saturating_sub(MARK_REACH)..=i).any(|j| mark(lines[j]).is_some());
        if !marked {
            unmarked += controls;
        }
    }
    (unmarked, names)
}

fn baseline(path: &Path) -> BTreeMap<String, usize> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    (text.lines())
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (f, n) = l.rsplit_once(' ')?;
            Some((f.to_string(), n.parse().ok()?))
        })
        .collect()
}

#[test]
fn every_control_says_how_an_assistant_reaches_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let allowed = baseline(&root.join("widgets-unmarked.txt"));
    let known = |name: &str| {
        name == "view"
            || crate::agent::tool(name).is_some()
            || crate::commands::Cmd::from_id(name).is_some()
    };
    let mut problems = Vec::new();
    let mut counts = BTreeMap::new();
    let mut files: Vec<_> = std::fs::read_dir(root.join("src"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // icons.rs builds the icon button; the places that show one carry
        // the marks.
        if name == "widget_check.rs" || name == "icons.rs" {
            continue;
        }
        let (unmarked, names) = scan(&std::fs::read_to_string(&path).unwrap());
        for (line, n) in names {
            if !known(&n) {
                problems.push(format!(
                    "{name}:{line}: \"// ai: {n}\" names no command or assistant tool"
                ));
            }
        }
        let was = allowed.get(&name).copied().unwrap_or(0);
        if unmarked > was {
            problems.push(format!(
                "{name}: {unmarked} controls without an \"// ai:\" mark, more than the {was} \
                 from before the rule. Mark each new one with the command or tool an \
                 assistant uses for it, or \"view\" (see widget_check.rs)."
            ));
        }
        if unmarked > 0 {
            counts.insert(name, unmarked);
        }
    }
    // Marking old controls lowers the counts; keep the list as low as
    // they are, so none can creep back.
    for (file, was) in &allowed {
        let now = counts.get(file).copied().unwrap_or(0);
        if now < *was {
            problems.push(format!(
                "{file}: now {now} unmarked controls, fewer than the {was} in \
                 widgets-unmarked.txt; lower it there"
            ));
        }
    }
    if !problems.is_empty() {
        let list: String = counts.iter().map(|(f, n)| format!("{f} {n}\n")).collect();
        panic!(
            "{}\n\nThe counts as they are now, for widgets-unmarked.txt:\n{list}",
            problems.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_found_above_a_control() {
        let src = "let a = 1;\n// ai: view\nif ui\n    .button(\"x\")\n    .clicked() {}\n\
                   ui.checkbox(&mut b, \"y\");\n";
        let (unmarked, names) = scan(src);
        assert_eq!(unmarked, 1, "the checkbox has no mark");
        assert_eq!(names, vec![(2, "view".to_string())]);
        let tests =
            "ui.button(\"a\"); // ai: add-bus\n#[cfg(test)]\nmod tests {\n ui.button(\"b\");\n}";
        assert_eq!(scan(tests), (0, vec![(1, "add-bus".to_string())]));
    }
}
