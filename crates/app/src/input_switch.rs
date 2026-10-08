//! Switching the keyboard input source with the focus (setting `auto_input_source`, on by
//! default): a terminal always starts in an ASCII source (English); the Agent composer gets
//! back the source last used in it, the user's non-English source to begin with. Other
//! places (the editor...) are left alone, and the user may switch freely inside an area.
//! Pure logic; `platform` reads and selects the sources, the workbench calls on focus
//! changes. App-wide: the input source is the system's, shared by every window.

/// A keyboard input source as the system names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// e.g. `com.apple.keylayout.ABC`, `com.tencent.inputmethod.wetype.pinyin`.
    pub id: String,
    /// Types ASCII as it is (an English layout, not an input method).
    pub ascii: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Terminal,
    Composer,
}

/// What to switch to.
#[derive(Debug, PartialEq, Eq)]
pub enum Switch {
    /// The ASCII source the system would pick (the user's English layout).
    Ascii,
    Select(String),
}

#[derive(Default)]
pub struct InputSwitch {
    /// The source in use when the composer last lost focus.
    composer: Option<String>,
    /// The last non-ASCII source seen in use: the composer's until it has its own.
    non_ascii: Option<String>,
}

impl gpui_kit::Global for InputSwitch {}

impl InputSwitch {
    /// Focus entered `area` while `current` is in use; `fallback` names an enabled
    /// non-ASCII source when none has been seen yet.
    pub fn enter(
        &mut self,
        area: Area,
        current: Option<&Source>,
        fallback: impl FnOnce() -> Option<String>,
    ) -> Option<Switch> {
        if let Some(current) = current.filter(|c| !c.ascii) {
            self.non_ascii = Some(current.id.clone());
        }
        match area {
            Area::Terminal => (!current.is_some_and(|c| c.ascii)).then_some(Switch::Ascii),
            Area::Composer => {
                let target = self
                    .composer
                    .clone()
                    .or_else(|| self.non_ascii.clone())
                    .or_else(fallback)?;
                (current.map(|c| &c.id) != Some(&target)).then_some(Switch::Select(target))
            }
        }
    }

    /// The composer lost focus (or its window did) while `current` was in use.
    pub fn leave_composer(&mut self, current: Option<&Source>) {
        let Some(current) = current else { return };
        self.composer = Some(current.id.clone());
        if !current.ascii {
            self.non_ascii = Some(current.id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abc() -> Source {
        Source {
            id: "abc".into(),
            ascii: true,
        }
    }

    fn pinyin() -> Source {
        Source {
            id: "pinyin".into(),
            ascii: false,
        }
    }

    #[test]
    fn terminals_always_start_in_english() {
        let mut s = InputSwitch::default();
        assert_eq!(
            s.enter(Area::Terminal, Some(&pinyin()), || None),
            Some(Switch::Ascii)
        );
        assert_eq!(s.enter(Area::Terminal, Some(&abc()), || None), None);
        // Unknown current source: switch anyway.
        assert_eq!(s.enter(Area::Terminal, None, || None), Some(Switch::Ascii));
    }

    #[test]
    fn the_composer_gets_back_what_was_used_in_it() {
        let mut s = InputSwitch::default();
        // First time: the non-English source seen before the terminal switched away.
        s.enter(Area::Terminal, Some(&pinyin()), || None);
        assert_eq!(
            s.enter(Area::Composer, Some(&abc()), || None),
            Some(Switch::Select("pinyin".into()))
        );
        assert_eq!(s.enter(Area::Composer, Some(&pinyin()), || None), None);
        // The user picked English in the composer: kept for it.
        s.leave_composer(Some(&abc()));
        assert_eq!(
            s.enter(Area::Composer, Some(&pinyin()), || None),
            Some(Switch::Select("abc".into()))
        );
    }

    #[test]
    fn with_nothing_seen_the_composer_uses_an_enabled_input_method() {
        let mut s = InputSwitch::default();
        assert_eq!(
            s.enter(Area::Composer, Some(&abc()), || Some("wetype".into())),
            Some(Switch::Select("wetype".into()))
        );
        assert_eq!(
            InputSwitch::default().enter(Area::Composer, Some(&abc()), || None),
            None
        );
    }
}
