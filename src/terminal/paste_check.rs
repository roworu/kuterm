//! characters in pasted text that hide what it really runs: zero-width and bidi control
//! characters, and letters of other scripts passing for latin ones (the "trojan source"
//! tricks)

/// true for chars that change how text reads without showing themselves
fn hidden(c: char) -> bool {
    matches!(c,
        // zero-width space, non-joiner, joiner, word joiner, byte order mark
        '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}'
        // bidi embeddings, overrides and isolates
        | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        // left-to-right and right-to-left marks
        | '\u{200e}' | '\u{200f}')
}

/// letters of scripts with lookalikes of latin ones, like cyrillic а and greek ο
fn lookalike_script(c: char) -> bool {
    matches!(c, '\u{0370}'..='\u{03ff}' | '\u{0400}'..='\u{052f}' | '\u{0530}'..='\u{058f}')
}

fn pictograph(c: char) -> bool {
    c as u32 >= 0x2000 && !c.is_alphanumeric() && !hidden(c)
}

/// byte offsets of the chars of `text` to point out: hidden chars, and lookalike letters in a
/// word that also has latin letters. words all in one script, cjk and emoji are fine
pub fn flagged(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut word: Vec<(usize, char)> = Vec::new();
    let check_word = |word: &mut Vec<(usize, char)>, found: &mut Vec<usize>| {
        if word.iter().any(|(_, c)| c.is_ascii_alphabetic()) {
            found.extend(
                word.iter()
                    .filter(|(_, c)| lookalike_script(*c))
                    .map(|(at, _)| *at),
            );
        }
        word.clear();
    };
    let mut chars = text.char_indices().peekable();
    let mut before = None;
    while let Some((at, c)) = chars.next() {
        let after = chars.peek().map(|(_, c)| *c);
        // a joiner between two pictographs builds one emoji, like a family
        let emoji_joiner =
            c == '\u{200d}' && before.is_some_and(pictograph) && after.is_some_and(pictograph);
        before = Some(c);
        if hidden(c) && !emoji_joiner {
            found.push(at);
        } else if c.is_alphanumeric() {
            word.push((at, c));
        } else {
            check_word(&mut word, &mut found);
        }
    }
    check_word(&mut word, &mut found);
    found.sort_unstable();
    found
}

#[cfg(test)]
mod tests {
    use super::flagged;

    /// chars of `text` that get flagged
    fn chars(text: &str) -> Vec<char> {
        flagged(text)
            .into_iter()
            .map(|at| text[at..].chars().next().unwrap())
            .collect()
    }

    #[test]
    fn zero_width_chars_are_flagged() {
        for c in ['\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}'] {
            assert_eq!(chars(&format!("rm{c} -rf")), [c], "{:x}", c as u32);
        }
    }

    #[test]
    fn bidi_controls_are_flagged() {
        for c in ['\u{202a}', '\u{202e}', '\u{2066}', '\u{2069}', '\u{200f}'] {
            assert_eq!(chars(&format!("ls{c}txt.exe")), [c], "{:x}", c as u32);
        }
    }

    #[test]
    fn lookalike_letters_in_latin_words_are_flagged() {
        // cyrillic а and greek ο in otherwise latin words
        assert_eq!(chars("curl https://pаypal.com"), ['а']);
        assert_eq!(chars("gοogle"), ['ο']);
    }

    #[test]
    fn plain_text_in_any_script_is_fine() {
        for text in [
            "ls -la /tmp && echo done",
            "echo привет мир",
            "echo γεια σου",
            "echo 日本語のテキスト",
            "echo 🎉 emoji ✨",
            "echo 👨\u{200d}👩\u{200d}👧 family",
            "git commit -m 'исправил bug'",
        ] {
            assert!(flagged(text).is_empty(), "{text}");
        }
    }
}
