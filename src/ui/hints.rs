//! hints mode: short labels over pattern matches on screen, typing a label runs its action

use crate::terminal::Link;

/// matches on screen with their labels, and the letters typed so far
pub struct Hints {
    links: Vec<Link>,
    labels: Vec<String>,
    typed: String,
}

/// what a typed key did
#[derive(Debug, PartialEq)]
pub enum HintKey {
    /// part of a label, more letters are needed
    Pending,
    /// a whole label, its link is picked
    Picked(Link),
    /// no label goes on like this, hints mode ends
    Cancel,
}

/// labels for `count` matches: single letters while they suffice, two letters after that.
/// all labels have the same length, so none is the start of another
pub fn labels(count: usize, alphabet: &[char]) -> Vec<String> {
    if count <= alphabet.len() {
        return alphabet.iter().take(count).map(char::to_string).collect();
    }
    alphabet
        .iter()
        .flat_map(|a| alphabet.iter().map(move |b| format!("{a}{b}")))
        .take(count)
        .collect()
}

impl Hints {
    /// hints for `links`, none when there is nothing to pick. matches past what two letter
    /// labels can name are left out
    pub fn new(mut links: Vec<Link>, alphabet: &str) -> Option<Self> {
        let alphabet: Vec<char> = alphabet.chars().collect();
        links.truncate(alphabet.len() * alphabet.len());
        if links.is_empty() {
            return None;
        }
        let labels = labels(links.len(), &alphabet);
        Some(Self {
            links,
            labels,
            typed: String::new(),
        })
    }

    /// take one typed key, named like gpui names keys
    pub fn key(&mut self, key: &str) -> HintKey {
        if key == "backspace" {
            self.typed.pop();
            return HintKey::Pending;
        }
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return HintKey::Cancel;
        };
        self.typed.push(c);
        if let Some(ix) = self.labels.iter().position(|label| *label == self.typed) {
            return HintKey::Picked(self.links.swap_remove(ix));
        }
        if self
            .labels
            .iter()
            .any(|label| label.starts_with(&self.typed))
        {
            HintKey::Pending
        } else {
            HintKey::Cancel
        }
    }

    /// matches whose label still fits what was typed, with the rest of their label
    pub fn shown(&self) -> impl Iterator<Item = (&Link, &str)> {
        self.links
            .iter()
            .zip(&self.labels)
            .filter_map(|(link, label)| Some((link, label.strip_prefix(self.typed.as_str())?)))
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::index::{Column, Line, Point};

    use super::{HintKey, Hints, labels};
    use crate::{settings::PatternAction, terminal::Link};

    fn link(column: usize) -> Link {
        let at = Point::new(Line(0), Column(column));
        Link {
            range: at..=at,
            rest: Vec::new(),
            target: column.to_string(),
            action: PatternAction::Copy,
            name: None,
        }
    }

    #[test]
    fn labels_use_one_letter_until_they_run_out() {
        let alphabet = ['a', 's', 'd'];
        assert_eq!(labels(2, &alphabet), ["a", "s"]);
        assert_eq!(labels(3, &alphabet), ["a", "s", "d"]);
        assert_eq!(labels(4, &alphabet), ["aa", "as", "ad", "sa"]);
    }

    #[test]
    fn typing_a_label_picks_its_link() {
        let mut hints = Hints::new((0..5).map(link).collect(), "asd").unwrap();
        assert_eq!(hints.shown().count(), 5);
        assert_eq!(hints.key("s"), HintKey::Pending);
        // only labels starting with s are left, shown without the typed letter
        let shown: Vec<&str> = hints.shown().map(|(_, rest)| rest).collect();
        assert_eq!(shown, ["a", "s"]);
        assert_eq!(hints.key("backspace"), HintKey::Pending);
        assert_eq!(hints.shown().count(), 5);
        hints.key("s");
        assert_eq!(hints.key("a"), HintKey::Picked(link(3)));
    }

    #[test]
    fn other_keys_cancel_and_nothing_to_hint_gives_none() {
        let mut hints = Hints::new(vec![link(0), link(1)], "asd").unwrap();
        assert_eq!(hints.key("x"), HintKey::Cancel);
        let mut hints = Hints::new(vec![link(0)], "asd").unwrap();
        assert_eq!(hints.key("enter"), HintKey::Cancel);
        assert!(Hints::new(Vec::new(), "asd").is_none());
        // more matches than two letters can name are left out
        let hints = Hints::new((0..20).map(link).collect(), "as").unwrap();
        assert_eq!(hints.shown().count(), 4);
    }
}
