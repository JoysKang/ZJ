//! Terminal focus selects an ASCII input source; other controls keep their input source.

/// A keyboard input source as the system names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub ascii: bool,
}

pub fn terminal_needs_ascii(current: Option<&Source>) -> bool {
    !current.is_some_and(|source| source.ascii)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_start_in_english_without_switching_an_existing_ascii_source() {
        assert!(terminal_needs_ascii(None));
        assert!(terminal_needs_ascii(Some(&Source {
            id: "pinyin".into(),
            ascii: false
        })));
        assert!(!terminal_needs_ascii(Some(&Source {
            id: "abc".into(),
            ascii: true
        })));
    }
}
