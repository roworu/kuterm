//! command line up to the cursor split into shell words, the way the shell would see them

/// the command the cursor is in, with the word being typed
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineContext {
    /// unquoted words of the command before the one being typed
    pub words: Vec<String>,
    /// unquoted text of the word being typed, up to the cursor
    pub prefix: String,
    /// char offset in the buffer where the typed word starts
    pub start: usize,
    /// quote still open in the typed word, none when it is unquoted
    pub quote: Option<char>,
    /// the typed word follows `>` or `<`, so it is a file
    pub redirect: bool,
    /// option of a typed `--name=value`, then `prefix` and `start` are of the value
    pub option: Option<String>,
}

/// split `line`, the buffer up to the cursor
pub fn parse(line: &[char]) -> LineContext {
    let mut context = LineContext::default();
    // word being read: start offset and unquoted text
    let mut word: Option<(usize, String)> = None;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut redirect = false;
    // the first unquoted `=` of a word that starts with `-`: word start, value start, and
    // the length of the text before it
    let mut equals: Option<(usize, usize, usize)> = None;

    let finish =
        |word: &mut Option<(usize, String)>, context: &mut LineContext, redirect: &mut bool| {
            if let Some((_, text)) = word.take() {
                // the target of a redirection is no argument of the command
                if !std::mem::take(redirect) {
                    context.words.push(text);
                }
            }
        };

    for (ix, &c) in line.iter().enumerate() {
        if escaped {
            escaped = false;
            // in double quotes a backslash only escapes what the quotes would expand
            let text = &mut word.get_or_insert_with(|| (ix - 1, String::new())).1;
            if quote == Some('"') && !matches!(c, '"' | '\\' | '$' | '`' | '\n') {
                text.push('\\');
            }
            // a backslash before a newline joins the lines
            if c != '\n' {
                text.push(c);
            }
            continue;
        }
        match quote {
            Some('\'') if c == '\'' => quote = None,
            Some('"') if c == '"' => quote = None,
            Some('"') if c == '\\' => escaped = true,
            Some(_) => word.get_or_insert_with(|| (ix, String::new())).1.push(c),
            None => match c {
                '\\' => {
                    word.get_or_insert_with(|| (ix, String::new()));
                    escaped = true;
                }
                '\'' | '"' => {
                    word.get_or_insert_with(|| (ix, String::new()));
                    quote = Some(c);
                }
                '=' if word.as_ref().is_some_and(|(start, text)| {
                    text.starts_with('-') && equals.is_none_or(|(at, ..)| at != *start)
                }) =>
                {
                    let (start, text) = word.as_mut().unwrap();
                    equals = Some((*start, ix + 1, text.len()));
                    text.push(c);
                }
                ' ' | '\t' => finish(&mut word, &mut context, &mut redirect),
                '>' | '<' => {
                    // the fd number of "2>" is no argument either
                    if word
                        .as_ref()
                        .is_some_and(|(_, text)| text.chars().all(|c| c.is_ascii_digit()))
                    {
                        word = None;
                    }
                    finish(&mut word, &mut context, &mut redirect);
                    redirect = true;
                }
                // "&>", ">&2" and "<&0" redirect, they start no new command
                '&' if line.get(ix + 1) == Some(&'>')
                    || (ix > 0 && matches!(line[ix - 1], '>' | '<')) =>
                {
                    finish(&mut word, &mut context, &mut redirect);
                }
                // each of these starts a new command
                '|' | '&' | ';' | '(' | ')' | '`' | '\n' | '{' | '}' => {
                    word = None;
                    redirect = false;
                    context.words.clear();
                }
                _ => word.get_or_insert_with(|| (ix, String::new())).1.push(c),
            },
        }
    }
    let (mut start, mut prefix) = word.unwrap_or((line.len(), String::new()));
    if let Some((_, value_start, len)) = equals.filter(|(at, ..)| *at == start) {
        context.option = Some(prefix[..len].to_string());
        prefix = prefix[len + 1..].to_string();
        start = value_start;
    }
    context.start = start;
    context.prefix = prefix;
    // only a quote the word opened with can be continued by a completion
    context.quote = quote.filter(|q| line.get(start) == Some(q));
    context.redirect = redirect;
    context
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_end(line: &str) -> LineContext {
        parse(&line.chars().collect::<Vec<_>>())
    }

    #[test]
    fn words_before_the_cursor_and_the_typed_one() {
        let context = at_end("git  checkout ma");
        assert_eq!(context.words, ["git", "checkout"]);
        assert_eq!(context.prefix, "ma");
        assert_eq!(context.start, 14);
        assert_eq!(context.quote, None);

        // a space at the end starts an empty word
        let context = at_end("ls ");
        assert_eq!(context.words, ["ls"]);
        assert_eq!((context.prefix.as_str(), context.start), ("", 3));
    }

    #[test]
    fn quotes_and_escapes_are_removed() {
        let context = at_end(r#"cat "my file" a\ b 'it'"s" "dir/sp"#);
        assert_eq!(context.words, ["cat", "my file", "a b", "its"]);
        assert_eq!(context.prefix, "dir/sp");
        assert_eq!(context.quote, Some('"'));
        assert_eq!(context.start, 27);

        let context = at_end(r"cd my\ fo");
        assert_eq!(context.prefix, "my fo");
        assert_eq!((context.start, context.quote), (3, None));

        // a quote closed again leaves nothing to continue
        let context = at_end("cd 'ab'c");
        assert_eq!((context.prefix.as_str(), context.quote), ("abc", None));
    }

    #[test]
    fn separators_start_a_new_command() {
        for line in [
            "make && gi",
            "ls | gi",
            "a; gi",
            "echo $(gi",
            "(gi",
            "x &gi",
        ] {
            let context = at_end(line);
            assert_eq!(context.words, Vec::<String>::new(), "{line}");
            assert_eq!(context.prefix, "gi", "{line}");
        }
        // inside quotes they are plain text
        let context = at_end("echo 'a|b' c");
        assert_eq!(context.words, ["echo", "a|b"]);
    }

    #[test]
    fn redirections_take_a_file() {
        let context = at_end("sort < in > ou");
        assert_eq!(context.words, ["sort"]);
        assert!(context.redirect);
        assert_eq!(context.prefix, "ou");
        let context = at_end("sort > out x");
        assert_eq!(context.words, ["sort"]);
        assert!(!context.redirect);
        // fd numbers and "&" in a redirection are neither arguments nor a new command
        for line in [
            "git 2>/dev/null lo",
            "git &>log lo",
            "git >&2 lo",
            "git 2>&1 lo",
        ] {
            let context = at_end(line);
            assert_eq!(context.words, ["git"], "{line}");
            assert_eq!(context.prefix, "lo", "{line}");
            assert!(!context.redirect, "{line}");
        }
        assert!(at_end("make 2> ou").redirect);
    }

    #[test]
    fn the_value_of_an_option_is_the_typed_word() {
        let context = at_end("ls --color=al");
        assert_eq!(context.option.as_deref(), Some("--color"));
        assert_eq!((context.prefix.as_str(), context.start), ("al", 11));
        let context = at_end("x --out=\"my d");
        assert_eq!(context.option.as_deref(), Some("--out"));
        assert_eq!(
            (context.prefix.as_str(), context.quote),
            ("my d", Some('"'))
        );
        // only an unquoted = of the word being typed counts
        assert_eq!(at_end("x --a=b c").option, None);
        assert_eq!(at_end("x '--a=b").option, None);
        assert_eq!(at_end("x FOO=b").option, None);
    }

    #[test]
    fn backslash_in_double_quotes_stays_before_plain_letters() {
        // like the shell: "a\nb" keeps its backslash, "a\"b" and "a\$b" lose it
        let context = at_end(r#"echo "a\nb" "a\"b" "a\$b" x"#);
        assert_eq!(context.words, ["echo", r"a\nb", r#"a"b"#, "a$b"]);
    }
}
