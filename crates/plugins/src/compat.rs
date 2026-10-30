// SPDX-License-Identifier: GPL-3.0-or-later
//! Newer versions of plugins standing in for older ones (roadmap 2.4).
//!
//! A plugin can declare that one of its classes replaces older classes
//! (an earlier version of itself, or the same plugin in an older
//! format), so that a project saved with the old one opens with the new
//! one, its settings loaded into it. VST 3 has two places for this: a
//! `Compatibility` list in the bundle's `Contents/Resources/
//! moduleinfo.json`, or, where a plugin has no such file, a class of
//! category [`COMPAT_CATEGORY`] that writes the same list on request.
//! Both are JSON5: comments and trailing commas are allowed.
//!
//! Each entry names the new class and the old ones it replaces by their
//! IDs as text (see [`cid_from_sdk_string`]).

#![allow(non_snake_case)]

use std::path::Path;

use vst3::ComWrapper;
use vst3::Steinberg::*;

use crate::com::MemStream;
use crate::module::Module;

/// The category of the class that lists what a module's classes replace.
pub const COMPAT_CATEGORY: &str = "Plugin Compatibility Class";

/// One class standing in for an older one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replaces {
    pub new: TUID,
    pub old: TUID,
}

/// A class ID from the text the VST 3 SDK writes for one: 32 hex
/// digits. On Windows, where IDs are laid out as COM GUIDs, the first
/// eight digits are a little-endian 32-bit number and the next two
/// groups of four 16-bit ones; elsewhere the digits are the bytes in
/// order.
pub fn cid_from_sdk_string(s: &str) -> Option<TUID> {
    let s = s.trim();
    if s.len() != 32 || !s.is_ascii() {
        return None;
    }
    let mut b = [0u8; 16];
    for (i, x) in b.iter_mut().enumerate() {
        *x = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    if cfg!(windows) {
        b[0..4].reverse();
        b[4..6].reverse();
        b[6..8].reverse();
    }
    Some(b.map(|x| x as _))
}

/// The reverse of [`cid_from_sdk_string`].
pub fn cid_to_sdk_string(cid: &TUID) -> String {
    let mut b = cid.map(|x| x as u8);
    if cfg!(windows) {
        b[0..4].reverse();
        b[4..6].reverse();
        b[6..8].reverse();
    }
    b.iter().map(|x| format!("{x:02X}")).collect()
}

// ---------------------------------------------------------------------------
// Reading JSON5, as much of it as these lists use

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Str(String),
    List(Vec<Value>),
    Map(Vec<(String, Value)>),
    /// Numbers, true, false, null: not needed here, kept as text.
    Other(String),
}

impl Value {
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

struct Json<'a> {
    s: &'a [u8],
    at: usize,
    depth: u32,
}

type Res<T> = Result<T, String>;

impl Json<'_> {
    /// Skips white space and comments.
    fn skip(&mut self) {
        loop {
            while self.at < self.s.len() && self.s[self.at].is_ascii_whitespace() {
                self.at += 1;
            }
            let rest = &self.s[self.at..];
            if rest.starts_with(b"//") {
                while self.at < self.s.len() && self.s[self.at] != b'\n' {
                    self.at += 1;
                }
            } else if rest.starts_with(b"/*") {
                match rest[2..].windows(2).position(|w| w == b"*/") {
                    Some(p) => self.at += p + 4,
                    None => self.at = self.s.len(),
                }
            } else {
                return;
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.at).copied()
    }

    fn string(&mut self, quote: u8) -> Res<String> {
        self.at += 1;
        let mut out = Vec::new();
        while let Some(&c) = self.s.get(self.at) {
            self.at += 1;
            match c {
                c if c == quote => return String::from_utf8(out).map_err(|e| e.to_string()),
                b'\\' => {
                    let e = *self.s.get(self.at).ok_or("text cut short")?;
                    self.at += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        // \uXXXX and the rest: kept as written, which is
                        // enough for class IDs.
                        b'u' => out.extend_from_slice(b"\\u"),
                        e => out.push(e),
                    }
                }
                c => out.push(c),
            }
        }
        Err("text cut short".into())
    }

    /// A bare word: an unquoted key, a number, true, false or null.
    fn word(&mut self) -> Res<String> {
        let start = self.at;
        while let Some(&c) = self.s.get(self.at) {
            if c.is_ascii_alphanumeric() || b"_$+-.".contains(&c) {
                self.at += 1;
            } else {
                break;
            }
        }
        if self.at == start {
            return Err(format!("unexpected character at {start}"));
        }
        Ok(String::from_utf8_lossy(&self.s[start..self.at]).into_owned())
    }

    fn key(&mut self) -> Res<String> {
        match self.peek() {
            Some(q @ (b'"' | b'\'')) => self.string(q),
            _ => self.word(),
        }
    }

    fn expect(&mut self, c: u8) -> Res<()> {
        if self.peek() == Some(c) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at {}", c as char, self.at))
        }
    }

    fn value(&mut self) -> Res<Value> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("nested too deeply".into());
        }
        let v = match self.peek().ok_or("ends too soon")? {
            q @ (b'"' | b'\'') => Value::Str(self.string(q)?),
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    if self.peek() == Some(b']') {
                        self.at += 1;
                        break;
                    }
                    items.push(self.value()?);
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {}
                        _ => return Err(format!("expected ',' or ']' at {}", self.at)),
                    }
                }
                Value::List(items)
            }
            b'{' => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    if self.peek() == Some(b'}') {
                        self.at += 1;
                        break;
                    }
                    let k = self.key()?;
                    self.expect(b':')?;
                    items.push((k, self.value()?));
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {}
                        _ => return Err(format!("expected ',' or '}}' at {}", self.at)),
                    }
                }
                Value::Map(items)
            }
            _ => Value::Other(self.word()?),
        };
        self.depth -= 1;
        Ok(v)
    }
}

fn parse(text: &str) -> Res<Value> {
    let mut j = Json {
        s: text.as_bytes(),
        at: 0,
        depth: 0,
    };
    let v = j.value()?;
    if j.peek().is_some() {
        return Err("more after the end".into());
    }
    Ok(v)
}

/// What a compatibility list says: a list of `{"New": id, "Old": [id,
/// …]}`. Entries or IDs that can't be read are left out.
fn entries(list: &Value) -> Vec<Replaces> {
    let Value::List(items) = list else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(Value::Str(new)) = item.get("New") else {
            continue;
        };
        let Some(new) = cid_from_sdk_string(new) else {
            continue;
        };
        let Some(Value::List(olds)) = item.get("Old") else {
            continue;
        };
        for old in olds {
            if let Value::Str(old) = old
                && let Some(old) = cid_from_sdk_string(old)
                && old != new
            {
                out.push(Replaces { new, old });
            }
        }
    }
    out
}

/// Reads a compatibility list on its own, as the compatibility class
/// writes it.
pub fn parse_list(text: &str) -> Res<Vec<Replaces>> {
    Ok(entries(&parse(text)?))
}

/// Reads the `Compatibility` list of a whole `moduleinfo.json`. None if
/// it has none.
pub fn parse_module_info(text: &str) -> Res<Option<Vec<Replaces>>> {
    Ok(parse(text)?.get("Compatibility").map(entries))
}

/// Where a bundle keeps its `moduleinfo.json`.
pub fn module_info_path(bundle: &Path) -> std::path::PathBuf {
    bundle
        .join("Contents")
        .join("Resources")
        .join("moduleinfo.json")
}

/// What the classes of `module` (loaded from its bundle) replace: from
/// its `moduleinfo.json` if it has one, else from its compatibility
/// class.
pub fn of_module(module: &Module) -> Vec<Replaces> {
    if let Ok(text) = std::fs::read_to_string(module_info_path(&module.path)) {
        // A moduleinfo.json takes precedence, as the VST 3 SDK says,
        // even when it lists nothing.
        if let Ok(Some(list)) = parse_module_info(&text) {
            return list;
        }
        if parse(&text).is_ok() {
            return Vec::new();
        }
    }
    let Some(class) = module
        .classes()
        .into_iter()
        .find(|c| c.category == COMPAT_CATEGORY)
    else {
        return Vec::new();
    };
    let Some(c) = module.create::<IPluginCompatibility>(&class.cid) else {
        return Vec::new();
    };
    let s = ComWrapper::new(MemStream::default());
    let p = s.to_com_ptr::<IBStream>().unwrap();
    if unsafe { c.getCompatibilityJSON(p.as_ptr()) } != kResultTrue {
        return Vec::new();
    }
    parse_list(&String::from_utf8_lossy(&s.bytes())).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example in the VST 3 SDK's description of moduleinfo.json.
    const SDK_EXAMPLE: &str = r#"{
  "Name": "",
  "Version": "1.0",
  "Factory Info": {
    "Vendor": "Steinberg Media Technologies",
    "URL": "http://www.steinberg.net",
    "E-Mail": "mailto:info@steinberg.de",
    "Flags": {
      "Unicode": true,
      "Classes Discardable": false,
      "Component Non Discardable": false,
    },
  },
  "Compatibility": [
    {
		"New": "B9F9ADE1CD9C4B6DA57E61E3123535FD",
		"Old": [
		  "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", // just an example
		  "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", // another example
		],
	},
  ],
  "Classes": [
    {
      "CID": "B9F9ADE1CD9C4B6DA57E61E3123535FD",
      "Category": "Audio Module Class",
      "Name": "AGainSimple VST3",
      "Sub Categories": [
        "Fx",
      ],
      "Class Flags": 0,
      "Cardinality": 2147483647,
      "Snapshots": [
      ],
    },
  ],
}"#;

    #[test]
    fn the_sdk_example_lists_two_old_classes() {
        let list = parse_module_info(SDK_EXAMPLE).unwrap().unwrap();
        let new = cid_from_sdk_string("B9F9ADE1CD9C4B6DA57E61E3123535FD").unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|r| r.new == new));
        assert_eq!(
            cid_to_sdk_string(&list[0].old),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
        assert_eq!(
            cid_to_sdk_string(&list[1].old),
            "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
        );
    }

    #[test]
    fn a_file_without_a_list_says_so() {
        assert_eq!(parse_module_info(r#"{"Name": "x"}"#).unwrap(), None);
        assert!(parse_module_info("{").is_err());
        assert!(parse_list("[{\"New\": 1}] x").is_err());
        assert_eq!(parse_list("[ /* none */ ]").unwrap(), vec![]);
    }

    #[test]
    fn ids_read_as_the_sdk_writes_them() {
        // The test plugin writes its IDs the way the SDK does; reading
        // them back gives the same bytes as the class itself has.
        let cid = daw_test_plugin::ROUTER_CID;
        let text = daw_test_plugin::sdk_string(&cid);
        assert_eq!(cid_from_sdk_string(&text), Some(cid));
        assert_eq!(cid_to_sdk_string(&cid), text);
        assert_eq!(cid_from_sdk_string("not hex at all, 32 characters!!"), None);
        assert_eq!(cid_from_sdk_string("AB"), None);
    }

    #[test]
    fn bad_entries_are_left_out() {
        let list = parse_list(
            r#"[
              {New: "B9F9ADE1CD9C4B6DA57E61E3123535FD", Old: ["short", 3, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"]},
              {"Old": ["BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"]},
              'not a map',
            ]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            cid_to_sdk_string(&list[0].old),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
    }
}
