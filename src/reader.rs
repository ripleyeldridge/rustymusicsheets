#[derive(Clone, Debug, Default)]
pub struct Reader {
    pub score_id: Option<i64>,
    pub page: u32,
    pub pages: u32,
    pub two_pages: bool,
    pub sequence: Vec<i64>,
    pub sequence_index: usize,
}

#[cfg(test)]
mod follow_tests {
    use super::*;

    #[test]
    fn synth_following_preserves_visible_spreads_and_supports_backward_repeats() {
        let mut reader = Reader {
            two_pages: true,
            ..Default::default()
        };
        reader.load(1, 5, 0);
        assert!(!reader.follow_page(1));
        assert!(reader.follow_page(2));
        assert!(!reader.follow_page(3));
        assert!(reader.follow_page(0));
        assert_eq!(reader.page, 0);
        assert!(reader.follow_page(4));
        assert!(!reader.follow_page(5));
        assert_eq!(reader.page, 4);
    }

    #[test]
    fn synth_following_never_advances_setlist_entries_or_clamps_invalid_pages() {
        let mut reader = Reader {
            sequence: vec![1, 2],
            ..Default::default()
        };
        assert!(!reader.follow_page(0));
        reader.load(1, 2, 0);
        assert!(reader.follow_page(1));
        assert!(!reader.follow_page(2));
        assert_eq!(reader.score_id, Some(1));
        assert_eq!(reader.sequence_index, 0);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    Page,
    Score(i64),
    Boundary,
}

impl Reader {
    pub fn retain_existing(&mut self, valid: &std::collections::HashSet<i64>) -> bool {
        if self.score_id.is_some_and(|id| !valid.contains(&id)) {
            *self = Self {
                two_pages: self.two_pages,
                ..Self::default()
            };
            return true;
        }
        let before = self
            .sequence
            .iter()
            .take(self.sequence_index)
            .filter(|id| valid.contains(id))
            .count();
        self.sequence.retain(|id| valid.contains(id));
        self.sequence_index = before.min(self.sequence.len().saturating_sub(1));
        false
    }
    pub fn load(&mut self, score: i64, pages: u32, page: u32) {
        self.score_id = Some(score);
        self.pages = pages;
        self.page = page.min(pages.saturating_sub(1));
    }

    pub fn jump(&mut self, page: u32) {
        self.page = page.min(self.pages.saturating_sub(1));
    }

    /// Synth following cannot cross scores or turn a page already on screen.
    pub fn follow_page(&mut self, page: u32) -> bool {
        if self.score_id.is_none()
            || page >= self.pages
            || page == self.page
            || (self.two_pages && self.page.checked_add(1) == Some(page))
        {
            return false;
        }
        self.page = page;
        true
    }

    pub fn can_turn(&self, forward: bool) -> bool {
        if self.score_id.is_none() || self.pages == 0 {
            return false;
        }
        if forward {
            self.page.saturating_add(if self.two_pages { 2 } else { 1 }) < self.pages
                || self.sequence_index.saturating_add(1) < self.sequence.len()
        } else {
            self.page > 0 || (self.sequence_index > 0 && !self.sequence.is_empty())
        }
    }

    pub fn turn(&mut self, forward: bool) -> Turn {
        if self.score_id.is_none() || self.pages == 0 {
            return Turn::Boundary;
        }
        let step = if self.two_pages { 2 } else { 1 };
        if forward {
            if self.page.saturating_add(step) < self.pages {
                self.page += step;
                return Turn::Page;
            }
            if self.sequence_index + 1 < self.sequence.len() {
                self.sequence_index += 1;
                return Turn::Score(self.sequence[self.sequence_index]);
            }
        } else {
            if self.page > 0 {
                self.page = self.page.saturating_sub(step);
                return Turn::Page;
            }
            if self.sequence_index > 0 && !self.sequence.is_empty() {
                self.sequence_index -= 1;
                return Turn::Score(self.sequence[self.sequence_index]);
            }
        }
        Turn::Boundary
    }
}
