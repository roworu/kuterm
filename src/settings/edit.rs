//! changing one value of a jsonc settings file in place, so its comments and layout stay

/// index after the whitespace and comments starting at `at`
fn skip_blank(text: &[u8], mut at: usize) -> usize {
    loop {
        match text.get(at..) {
            Some([b' ' | b'\t' | b'\n' | b'\r', ..]) => at += 1,
            Some([b'/', b'/', ..]) => {
                while at < text.len() && text[at] != b'\n' {
                    at += 1;
                }
            }
            Some([b'/', b'*', ..]) => {
                at += 2;
                while at < text.len() && !text[at..].starts_with(b"*/") {
                    at += 1;
                }
                at = (at + 2).min(text.len());
            }
            _ => return at,
        }
    }
}

/// index after the string starting at `at`, which is its opening quote
fn skip_string(text: &[u8], mut at: usize) -> Option<usize> {
    at += 1;
    while at < text.len() {
        match text[at] {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

/// index after the value starting at `at`
fn skip_value(text: &[u8], at: usize) -> Option<usize> {
    match text.get(at)? {
        b'"' => skip_string(text, at),
        b'{' | b'[' => {
            let mut depth = 0;
            let mut at = at;
            while at < text.len() {
                at = skip_blank(text, at);
                match text.get(at)? {
                    b'"' => {
                        at = skip_string(text, at)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(at + 1);
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
            None
        }
        // numbers, true, false and null run up to the next separator
        _ => Some(
            at + text[at..]
                .iter()
                .position(|b| matches!(b, b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t' | b'/'))
                .unwrap_or(text.len() - at),
        ),
    }
}

/// where the value of `key` sits in the object whose `{` is at `open`
fn member(text: &[u8], open: usize, key: &str) -> Option<(usize, usize)> {
    let mut at = open + 1;
    loop {
        at = skip_blank(text, at);
        match text.get(at)? {
            b'}' => return None,
            b',' => {
                at += 1;
                continue;
            }
            b'"' => {}
            _ => return None,
        }
        let end = skip_string(text, at)?;
        let name = std::str::from_utf8(&text[at + 1..end - 1]).ok()?;
        at = skip_blank(text, end);
        if text.get(at) != Some(&b':') {
            return None;
        }
        let start = skip_blank(text, at + 1);
        let value_end = skip_value(text, start)?;
        if name == key {
            return Some((start, value_end));
        }
        at = value_end;
    }
}

/// `text` with the string at `path` set to `value`, objects on the way added when missing.
/// none when the file is no object
pub fn set_string(text: &str, path: &[&str], value: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut open = skip_blank(bytes, 0);
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let quoted = serde_json_lenient::to_string(value).ok()?;
    for (depth, key) in path.iter().enumerate() {
        match member(bytes, open, key) {
            Some((start, end)) if depth + 1 == path.len() => {
                return Some(format!("{}{quoted}{}", &text[..start], &text[end..]));
            }
            Some((start, _)) if bytes[start] == b'{' => open = start,
            // a missing key, or no object to go into, gets the rest of the path as new members
            found => {
                let mut inserted = quoted.clone();
                for key in path[depth + 1..].iter().rev() {
                    inserted = format!(
                        "{{ {}: {inserted} }}",
                        serde_json_lenient::to_string(key).ok()?
                    );
                }
                let key = serde_json_lenient::to_string(key).ok()?;
                return Some(match found {
                    Some((start, end)) => format!("{}{inserted}{}", &text[..start], &text[end..]),
                    None => {
                        let indent = "  ".repeat(depth + 1);
                        format!(
                            "{}\n{indent}{key}: {inserted},{}",
                            &text[..open + 1],
                            &text[open + 1..]
                        )
                    }
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::set_string;

    #[test]
    fn existing_value_is_replaced_and_comments_stay() {
        let text = r##"{
  // the look
  "theme": {
    "mode": "dark", // keeps this
    "themes": { "dark": { "text": "#fff" } },
    "dark": "dark",
  },
}"##;
        let changed = set_string(text, &["theme", "dark"], "nord").unwrap();
        assert!(changed.contains(r#""dark": "nord","#));
        // the nested theme of the same name is left alone
        assert!(changed.contains(r##""themes": { "dark": { "text": "#fff" } }"##));
        assert!(changed.contains("// the look") && changed.contains("// keeps this"));
        let value: serde_json_lenient::Value = serde_json_lenient::from_str(&changed).unwrap();
        assert_eq!(value["theme"]["dark"], "nord");
    }

    #[test]
    fn missing_keys_are_added() {
        let changed = set_string("{\n  \"font_size\": 12,\n}\n", &["theme", "light"], "x").unwrap();
        let value: serde_json_lenient::Value = serde_json_lenient::from_str(&changed).unwrap();
        assert_eq!(value["theme"]["light"], "x");
        assert_eq!(value["font_size"], 12);
        let changed =
            set_string(r#"{"theme": {"mode": "light"}}"#, &["theme", "light"], "y").unwrap();
        let value: serde_json_lenient::Value = serde_json_lenient::from_str(&changed).unwrap();
        assert_eq!(value["theme"]["light"], "y");
        assert_eq!(value["theme"]["mode"], "light");
        // a value that is no object is replaced by one
        let changed = set_string(r#"{"theme": null}"#, &["theme", "dark"], "z").unwrap();
        let value: serde_json_lenient::Value = serde_json_lenient::from_str(&changed).unwrap();
        assert_eq!(value["theme"]["dark"], "z");
        assert_eq!(set_string("[]", &["theme"], "z"), None);
    }
}
