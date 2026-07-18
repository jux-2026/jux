use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisibleItem {
    pub index: usize,
    pub area: Rect,
    pub skip_top: usize,
}

pub trait VirtualItemRenderer<T> {
    fn revision(&self, item: &T) -> u64;

    fn measure(&mut self, item: &T, width: u16) -> usize;

    fn render(&mut self, item: &T, index: usize, area: Rect, skip_top: usize, buffer: &mut Buffer);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VirtualListState {
    pub scroll_y: usize,
    pub measured_width: u16,
    pub item_heights: Vec<usize>,
    pub stick_to_bottom: bool,
    item_revisions: Vec<u64>,
    viewport_height: usize,
}

impl Default for VirtualListState {
    fn default() -> Self {
        Self {
            scroll_y: 0,
            measured_width: 0,
            item_heights: Vec::new(),
            stick_to_bottom: true,
            item_revisions: Vec::new(),
            viewport_height: 0,
        }
    }
}

impl VirtualListState {
    pub fn use_cached_measurements(
        &mut self,
        width: u16,
        viewport_height: u16,
        item_heights: Vec<usize>,
        scroll_y: usize,
        stick_to_bottom: bool,
    ) {
        self.measured_width = width;
        self.viewport_height = usize::from(viewport_height);
        self.item_revisions.clear();
        self.item_heights = item_heights;
        self.scroll_y = scroll_y.min(self.total_height());
        self.stick_to_bottom = stick_to_bottom;
    }

    pub fn measure<T, R: VirtualItemRenderer<T>>(
        &mut self,
        items: &[T],
        width: u16,
        viewport_height: u16,
        renderer: &mut R,
    ) {
        let width_changed = self.measured_width != width;
        self.measured_width = width;
        self.viewport_height = usize::from(viewport_height);
        self.item_heights.resize(items.len(), 0);
        self.item_revisions.resize(items.len(), u64::MAX);

        for (index, item) in items.iter().enumerate() {
            let revision = renderer.revision(item);
            if width_changed
                || self.item_heights[index] == 0
                || self.item_revisions[index] != revision
            {
                self.item_heights[index] = renderer.measure(item, width).max(1);
                self.item_revisions[index] = revision;
            }
        }

        if self.stick_to_bottom {
            self.scroll_to_bottom();
        } else {
            self.scroll_y = self.scroll_y.min(self.max_scroll());
        }
    }

    pub fn total_height(&self) -> usize {
        self.item_heights.iter().sum()
    }

    pub fn max_scroll(&self) -> usize {
        self.total_height().saturating_sub(self.viewport_height)
    }

    pub fn scroll_by(&mut self, delta: i32) {
        if delta < 0 {
            self.scroll_y = self.scroll_y.saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.scroll_y = self
                .scroll_y
                .saturating_add(delta as usize)
                .min(self.max_scroll());
        }
        self.stick_to_bottom = self.scroll_y == self.max_scroll();
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_y = 0;
        self.stick_to_bottom = self.max_scroll() == 0;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_y = self.max_scroll();
        self.stick_to_bottom = true;
    }

    pub fn visible_items(&self, area: Rect, overscan_rows: usize) -> Vec<VisibleItem> {
        if area.is_empty() || self.item_heights.is_empty() {
            return Vec::new();
        }
        let viewport_start = self.scroll_y;
        let viewport_end = viewport_start
            .saturating_add(usize::from(area.height))
            .saturating_add(overscan_rows)
            .min(self.total_height());
        let mut item_top = 0usize;
        let mut visible = Vec::new();

        for (index, height) in self.item_heights.iter().copied().enumerate() {
            let item_bottom = item_top.saturating_add(height);
            if item_bottom <= viewport_start {
                item_top = item_bottom;
                continue;
            }
            if item_top >= viewport_end {
                break;
            }
            let skip_top = viewport_start.saturating_sub(item_top);
            let visible_height = height
                .saturating_sub(skip_top)
                .min(viewport_end.saturating_sub(item_top.max(viewport_start)));
            let viewport_y = item_top.saturating_sub(viewport_start);
            visible.push(VisibleItem {
                index,
                area: Rect::new(
                    area.x,
                    area.y
                        .saturating_add(u16::try_from(viewport_y).unwrap_or(u16::MAX)),
                    area.width,
                    u16::try_from(visible_height).unwrap_or(u16::MAX),
                ),
                skip_top,
            });
            item_top = item_bottom;
        }
        visible
    }

    pub fn render<T, R: VirtualItemRenderer<T>>(
        &self,
        items: &[T],
        area: Rect,
        overscan_rows: usize,
        renderer: &mut R,
        buffer: &mut Buffer,
    ) {
        for visible in self.visible_items(area, overscan_rows) {
            renderer.render(
                &items[visible.index],
                visible.index,
                visible.area,
                visible.skip_top,
                buffer,
            );
        }
    }
}
