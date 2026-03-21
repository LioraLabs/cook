use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Running,
    Completed,
    Failed,
    Cached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Running,
    Completed,
    Failed,
    Cached,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct ActiveItem {
    pub label: String,
    pub status: ItemStatus,
}

#[derive(Debug, Clone, Copy)]
pub struct CacheInfo {
    pub hits: usize,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct Footer {
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub id: String,
    pub label: String,
    pub status: Status,
    pub progress: Option<(usize, usize)>,
    pub elapsed: Option<Duration>,
    pub active_items: Vec<ActiveItem>,
    pub cache_info: Option<CacheInfo>,
}

impl Section {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: Status::Waiting,
            progress: None,
            elapsed: None,
            active_items: Vec::new(),
            cache_info: None,
        }
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }

    pub fn progress(mut self, completed: usize, total: usize) -> Self {
        self.progress = Some((completed, total));
        self
    }

    pub fn elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = Some(elapsed);
        self
    }

    pub fn active_item(mut self, label: impl Into<String>, status: ItemStatus) -> Self {
        self.active_items.push(ActiveItem {
            label: label.into(),
            status,
        });
        self
    }

    pub fn cache(mut self, hits: usize, total: usize) -> Self {
        self.cache_info = Some(CacheInfo { hits, total });
        self
    }

}

#[derive(Debug, Clone)]
pub struct Frame {
    pub sections: Vec<Section>,
    pub footer: Option<Footer>,
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl Frame {
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
            footer: None,
        }
    }

    pub fn section(mut self, section: Section) -> Self {
        self.sections.push(section);
        self
    }

    pub fn footer(mut self, text: impl Into<String>) -> Self {
        self.footer = Some(Footer { text: text.into() });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn section_builder_defaults() {
        let s = Section::new("build", "Build");
        assert_eq!(s.id, "build");
        assert_eq!(s.label, "Build");
        assert_eq!(s.status, Status::Waiting);
        assert_eq!(s.progress, None);
        assert_eq!(s.elapsed, None);
        assert!(s.active_items.is_empty());
        assert!(s.cache_info.is_none());
    }

    #[test]
    fn section_builder_chaining() {
        let s = Section::new("lib", "lib")
            .status(Status::Running)
            .progress(3, 5)
            .elapsed(Duration::from_millis(800))
            .active_item("compile a.c", ItemStatus::Running)
            .active_item("compile b.c", ItemStatus::Running);

        assert_eq!(s.status, Status::Running);
        assert_eq!(s.progress, Some((3, 5)));
        assert_eq!(s.elapsed, Some(Duration::from_millis(800)));
        assert_eq!(s.active_items.len(), 2);
        assert_eq!(s.active_items[0].label, "compile a.c");
    }

    #[test]
    fn section_with_cache_info() {
        let s = Section::new("lib", "lib")
            .status(Status::Completed)
            .cache(3, 5);

        let info = s.cache_info.unwrap();
        assert_eq!(info.hits, 3);
        assert_eq!(info.total, 5);
    }

    #[test]
    fn frame_builder() {
        let frame = Frame::new()
            .section(Section::new("lib", "lib").status(Status::Running))
            .section(Section::new("test", "test").status(Status::Waiting))
            .footer("2 running · 1 waiting");

        assert_eq!(frame.sections.len(), 2);
        assert!(frame.footer.is_some());
        assert_eq!(frame.footer.unwrap().text, "2 running · 1 waiting");
    }

    #[test]
    fn frame_without_footer() {
        let frame = Frame::new()
            .section(Section::new("lib", "lib"));
        assert!(frame.footer.is_none());
    }

}
