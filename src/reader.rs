#[derive(Clone, Debug, Default)]
pub struct Reader {
    pub score_id: Option<i64>,
    pub page: u32,
    pub pages: u32,
    pub two_pages: bool,
    pub sequence: Vec<i64>,
    pub sequence_index: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    Page,
    Score(i64),
    Boundary,
}

impl Reader {
    pub fn load(&mut self, score: i64, pages: u32, page: u32) {
        self.score_id = Some(score);
        self.pages = pages;
        self.page = page.min(pages.saturating_sub(1));
    }

    pub fn jump(&mut self, page: u32) {
        self.page = page.min(self.pages.saturating_sub(1));
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
