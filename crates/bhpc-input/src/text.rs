//! The host's model of the focused text field: UTF-16 units (the unit of `NSRange`) and a caret.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextModel {
    units: Vec<u16>,
    caret: usize,
}

impl TextModel {
    /// A model holding `text` with the caret at the end.
    pub fn new(text: &str) -> TextModel {
        let units: Vec<u16> = text.encode_utf16().collect();
        TextModel { caret: units.len(), units }
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn text(&self) -> String {
        String::from_utf16_lossy(&self.units)
    }

    /// Range `(location, length)` that Backspace would delete, or `None` at the start.
    /// A surrogate pair counts as one character.
    pub fn back_range(&self) -> Option<(usize, usize)> {
        if self.caret == 0 {
            return None;
        }
        let pair = self.caret >= 2 && is_low(self.units[self.caret - 1]) && is_high(self.units[self.caret - 2]);
        let len = if pair { 2 } else { 1 };
        Some((self.caret - len, len))
    }

    /// Range that the Delete key would remove, or `None` at the end.
    pub fn forward_range(&self) -> Option<(usize, usize)> {
        if self.caret >= self.units.len() {
            return None;
        }
        let pair = is_high(self.units[self.caret]) && self.caret + 1 < self.units.len();
        Some((self.caret, if pair { 2 } else { 1 }))
    }

    /// Replace `[location, location+length)` with `with` and put the caret after it. The range is
    /// clamped to the text.
    pub fn replace(&mut self, location: usize, length: usize, with: &str) {
        let start = location.min(self.units.len());
        let end = (start + length).min(self.units.len());
        let new: Vec<u16> = with.encode_utf16().collect();
        self.caret = start + new.len();
        self.units.splice(start..end, new);
    }

    /// Take over the field's text when the game changed it itself (it upper-cases and filters what is typed, sets
    /// the result and answers NO). The caret moves by the change in length, so it stays behind what was typed.
    pub fn sync(&mut self, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        if units == self.units {
            return;
        }
        let grown = units.len() as isize - self.units.len() as isize;
        let mut caret = (self.caret as isize + grown).clamp(0, units.len() as isize) as usize;
        // Never between the halves of a surrogate pair.
        if caret > 0 && caret < units.len() && is_low(units[caret]) && is_high(units[caret - 1]) {
            caret += 1;
        }
        (self.units, self.caret) = (units, caret);
    }

    pub fn left(&mut self) {
        if let Some((loc, _)) = self.back_range() {
            self.caret = loc;
        }
    }

    pub fn right(&mut self) {
        if let Some((loc, len)) = self.forward_range() {
            self.caret = loc + len;
        }
    }

    pub fn home(&mut self) {
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.caret = self.units.len();
    }
}

fn is_high(u: u16) -> bool {
    (0xD800..0xDC00).contains(&u)
}

fn is_low(u: u16) -> bool {
    (0xDC00..0xE000).contains(&u)
}
