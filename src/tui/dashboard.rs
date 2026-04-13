use super::bar;
use super::detail::DetailView;
use crate::providers::{ProviderKind, ProviderResult, ProviderStatus, QuotaWindow};
use ratatui::layout::Rect;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::cell::{Cell, RefCell};

#[derive(Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Result of a mouse hit-test over the dashboard.
pub enum HitResult {
    /// A provider card at the given visual position was clicked.
    Card(usize),
    /// The Refresh button was clicked.
    Refresh,
    /// The Quit button was clicked.
    Quit,
}

#[derive(Clone)]
pub enum ProviderEntry {
    Loading,
    /// Stale data from a previous fetch; a refresh is in-flight.
    /// The card renders the old data with a spinner overlay so users
    /// don't lose context during refresh.
    Refreshing(ProviderResult),
    Done(ProviderResult),
}

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const MIN_CARD_W: u16 = 28;
const MIN_CARD_H: u16 = 7;

#[derive(Clone, Copy, Default)]
struct GridLayout {
    cols: usize,
    per_page: usize,
}

pub struct Dashboard {
    pub kinds: Vec<ProviderKind>,
    pub entries: Vec<ProviderEntry>,
    pub selected_index: usize,
    pub show_detail: bool,
    pub detail_scroll: u16,
    pub spinner_frame: usize,
    last_layout: Cell<GridLayout>,
    /// Cached visual order, locked in when all entries are loaded.
    /// Reset to identity order on first load; refreshes don't reshuffle.
    stable_order: Vec<usize>,
    // --- mouse support (interior-mutable so render(&self) can update) ---
    mouse_pos: Cell<Option<(u16, u16)>>,
    /// (card_area, visual_pos) for each card rendered on the last frame.
    card_hit_areas: RefCell<Vec<(Rect, usize)>>,
    refresh_btn: Cell<Option<Rect>>,
    quit_btn: Cell<Option<Rect>>,
}

impl Dashboard {
    pub fn new_loading(kinds: Vec<ProviderKind>) -> Self {
        let n = kinds.len();
        let entries = kinds.iter().map(|_| ProviderEntry::Loading).collect();
        Self {
            kinds,
            entries,
            selected_index: 0,
            show_detail: false,
            detail_scroll: 0,
            spinner_frame: 0,
            last_layout: Cell::new(GridLayout::default()),
            stable_order: (0..n).collect(),
            mouse_pos: Cell::new(None),
            card_hit_areas: RefCell::new(Vec::new()),
            refresh_btn: Cell::new(None),
            quit_btn: Cell::new(None),
        }
    }

    /// Update the last-known mouse position (used for button hover styling).
    pub fn set_mouse_pos(&self, col: u16, row: u16) {
        self.mouse_pos.set(Some((col, row)));
    }

    /// Hit-test a mouse click at `(col, row)`.
    /// Returns the action the click should trigger, if any.
    pub fn hit_test(&self, col: u16, row: u16) -> Option<HitResult> {
        let in_rect = |r: Rect| -> bool {
            col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
        };
        if self.refresh_btn.get().is_some_and(in_rect) {
            return Some(HitResult::Refresh);
        }
        if self.quit_btn.get().is_some_and(in_rect) {
            return Some(HitResult::Quit);
        }
        for &(area, vpos) in self.card_hit_areas.borrow().iter() {
            if in_rect(area) {
                return Some(HitResult::Card(vpos));
            }
        }
        None
    }

    /// Whether the entry at `idx` (by kinds index) has a completed result.
    pub fn is_entry_done(&self, idx: usize) -> bool {
        self.entries
            .get(idx)
            .is_some_and(|e| matches!(e, ProviderEntry::Done(_)))
    }

    /// Reset a single entry to Refreshing (keeps old data). Used for
    /// per-provider auto-refresh so each provider has its own cadence.
    pub fn reset_one(&mut self, idx: usize) {
        if let Some(e) = self.entries.get_mut(idx) {
            let old = std::mem::replace(e, ProviderEntry::Loading);
            *e = match old {
                ProviderEntry::Done(r) | ProviderEntry::Refreshing(r) => {
                    ProviderEntry::Refreshing(r)
                }
                ProviderEntry::Loading => ProviderEntry::Loading,
            };
        }
    }

    pub fn scroll_detail(&mut self, delta: i16) {
        self.detail_scroll = (self.detail_scroll as i32 + delta as i32).max(0) as u16;
    }

    /// Navigate to the previous/next provider while in detail view, cycling
    /// through the visual order. Resets the scroll offset on each jump.
    pub fn detail_prev(&mut self) {
        let order = self.visual_order();
        if let Some(pos) = order.iter().position(|&i| i == self.selected_index) {
            if pos > 0 {
                self.selected_index = order[pos - 1];
            } else {
                self.selected_index = order[order.len() - 1];
            }
        }
        self.detail_scroll = 0;
    }

    pub fn detail_next(&mut self) {
        let order = self.visual_order();
        if let Some(pos) = order.iter().position(|&i| i == self.selected_index) {
            if pos + 1 < order.len() {
                self.selected_index = order[pos + 1];
            } else {
                self.selected_index = order[0];
            }
        }
        self.detail_scroll = 0;
    }

    pub fn update(&mut self, idx: usize, result: ProviderResult) {
        if idx < self.entries.len() {
            self.entries[idx] = ProviderEntry::Done(result);
        }
        // Freeze the visual order once all results are in, so subsequent
        // refreshes (which temporarily set entries back to Loading) don't
        // scramble the card positions.
        if self.all_loaded() {
            self.stable_order = self.compute_visual_order();
            // Clamp selected_index into the (possibly smaller) visible set.
            if self.selected_index >= self.stable_order.len() {
                self.selected_index = self.stable_order.len().saturating_sub(1);
            }
        }
    }

    pub fn reset_loading(&mut self) {
        for e in &mut self.entries {
            // Keep old data visible as Refreshing; only truly unloaded entries
            // stay as Loading.  stable_order is preserved so cards don't move.
            let old = std::mem::replace(e, ProviderEntry::Loading);
            *e = match old {
                ProviderEntry::Done(r) | ProviderEntry::Refreshing(r) => {
                    ProviderEntry::Refreshing(r)
                }
                ProviderEntry::Loading => ProviderEntry::Loading,
            };
        }
    }

    pub fn tick_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len();
    }

    pub fn all_loaded(&self) -> bool {
        self.entries
            .iter()
            .all(|e| matches!(e, ProviderEntry::Done(_)))
    }

    pub fn selected_provider(&self) -> Option<&ProviderResult> {
        let order = self.visual_order();
        let entry_idx = *order.get(self.selected_index)?;
        match self.entries.get(entry_idx)? {
            ProviderEntry::Done(r) | ProviderEntry::Refreshing(r) => Some(r),
            ProviderEntry::Loading => None,
        }
    }

    /// Returns the cached visual order (frozen when all entries last loaded).
    /// Using a stable cache means refreshes don't scramble card positions.
    fn visual_order(&self) -> Vec<usize> {
        self.stable_order.clone()
    }

    /// Number of providers currently visible in the main grid
    /// (excludes AuthRequired entries, which are shown only in the footer).
    fn visible_count(&self) -> usize {
        self.stable_order.len()
    }

    /// True when the entry at `idx` has AuthRequired status.
    fn is_auth_required_entry(entry: &ProviderEntry) -> bool {
        matches!(
            entry,
            ProviderEntry::Done(r) | ProviderEntry::Refreshing(r)
            if matches!(r.status, ProviderStatus::AuthRequired)
        )
    }

    /// Compute visual order by weight, excluding AuthRequired providers.
    /// Called only when all entries are Done.
    fn compute_visual_order(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.entries.len())
            .filter(|&i| !Self::is_auth_required_entry(&self.entries[i]))
            .collect();
        indices.sort_by_key(|&i| Self::card_weight(&self.entries[i]));
        indices
    }

    fn current_page(&self) -> usize {
        let layout = self.last_layout.get();
        if layout.per_page == 0 {
            0
        } else {
            self.selected_index / layout.per_page
        }
    }

    pub fn navigate(&mut self, dir: Direction) {
        if self.entries.is_empty() {
            return;
        }
        let layout = self.last_layout.get();
        if layout.cols == 0 || layout.per_page == 0 {
            return;
        }
        let total = self.visible_count();
        if total == 0 {
            return;
        }
        let page = self.current_page();
        let page_start = page * layout.per_page;
        let page_end = (page_start + layout.per_page).min(total);
        let page_count = page_end - page_start;
        let in_page_idx = self.selected_index - page_start;
        let cols = layout.cols.min(page_count.max(1));
        let rows_in_page = page_count.div_ceil(cols);
        let col = in_page_idx % cols;
        let row = in_page_idx / cols;

        match dir {
            Direction::Left => {
                if col > 0 {
                    self.selected_index -= 1;
                } else if page > 0 {
                    let prev_end = page * layout.per_page;
                    self.selected_index = prev_end - 1;
                }
            }
            Direction::Right => {
                if in_page_idx + 1 < page_count && col + 1 < cols {
                    self.selected_index += 1;
                } else if page_end < total {
                    self.selected_index = page_end;
                }
            }
            Direction::Up => {
                if row > 0 {
                    self.selected_index -= cols;
                }
            }
            Direction::Down => {
                if row + 1 < rows_in_page {
                    let candidate = self.selected_index + cols;
                    if candidate < page_end {
                        self.selected_index = candidate;
                    } else {
                        self.selected_index = page_end - 1;
                    }
                }
            }
        }
    }

    pub fn render(&self, f: &mut Frame) {
        if self.show_detail {
            self.render_detail(f);
        } else {
            self.render_grid(f);
        }
    }

    /// Assigns each card in `page_order` a (row, col, span) placement.
    /// MiniMax spans 2 columns; all other cards span 1.
    /// Cards wrap to the next row when they would overflow `cols`.
    fn flow_placements(
        entries: &[ProviderEntry],
        page_order: &[usize],
        cols: usize,
    ) -> Vec<(usize, usize, usize)> {
        let mut out = Vec::with_capacity(page_order.len());
        let mut row = 0usize;
        let mut col = 0usize;

        for &entry_idx in page_order {
            let span = match &entries[entry_idx] {
                ProviderEntry::Done(r) | ProviderEntry::Refreshing(r)
                    if r.kind == ProviderKind::Minimax =>
                {
                    2
                }
                _ => 1,
            }
            .min(cols); // Never wider than the grid itself.

            // Wrap to next row if the card won't fit.
            if col > 0 && col + span > cols {
                row += 1;
                col = 0;
            }

            out.push((row, col, span));
            col += span;

            // End of row — advance.
            if col >= cols {
                row += 1;
                col = 0;
            }
        }

        out
    }

    fn compute_layout(&self, grid_area: Rect) -> GridLayout {
        let n = self.visible_count().max(1);
        let max_cols = (grid_area.width / MIN_CARD_W).max(1) as usize;
        let max_rows = (grid_area.height / MIN_CARD_H).max(1) as usize;

        // Helper: actual flow rows needed for the first `k` entries at `cols`.
        let flow_rows_for = |cols: usize, k: usize| -> usize {
            let total = k.min(self.stable_order.len());
            if total == 0 || cols == 0 {
                return 1;
            }
            Self::flow_placements(&self.entries, &self.stable_order[..total], cols)
                .iter()
                .map(|(r, _, _)| r + 1)
                .max()
                .unwrap_or(1)
        };

        let flow_rows = |cols: usize| flow_rows_for(cols, n);

        // Find the largest page size where:
        //   (a) flow rows ≤ max_rows, AND
        //   (b) sum of per-row natural heights ≤ grid_area.height * 1.2
        //
        // The height check prevents over-squeezing cards when MiniMax-like
        // cards inflate a page beyond what looks good.  We allow a 20%
        // over-budget so a single unavailable card doesn't push neighbours to
        // the next page when they'd still render fine.
        let find_fitting_per_page = |cols: usize| -> usize {
            let mut best = 1usize;
            for k in 1..=n.min(self.stable_order.len()) {
                let slice = &self.stable_order[..k];
                let placements = Self::flow_placements(&self.entries, slice, cols);
                let nr = placements.iter().map(|(r, _, _)| r + 1).max().unwrap_or(0);
                if nr > max_rows {
                    break;
                }
                // Sum of max natural height per row.
                let mut row_h: Vec<u16> = vec![MIN_CARD_H; nr];
                for (i, &entry_idx) in slice.iter().enumerate() {
                    let (row_i, _, _) = placements[i];
                    let h = Self::natural_card_height(&self.entries[entry_idx]);
                    if h > row_h[row_i] {
                        row_h[row_i] = h;
                    }
                }
                let total_h: u32 = row_h.iter().map(|h| *h as u32).sum();
                // Accept if heights fit within 120% of available grid height.
                if total_h * 5 <= grid_area.height as u32 * 6 {
                    best = k;
                } else {
                    break;
                }
            }
            best.max(1)
        };

        let per_page = (max_cols * max_rows).max(1);

        if n <= per_page {
            // Choose cols balancing terminal aspect.
            // Aim for roughly square cells visually: chars are ~2:1 tall:wide,
            // so pick cols so that cols/rows ≈ grid_w / (grid_h * 2).
            let aspect = (grid_area.width as f64) / (grid_area.height as f64 * 2.0).max(1.0);
            let mut best_cols = 1usize.max(n.min(max_cols));
            let mut best_score = f64::MAX;
            for c in 1..=n.min(max_cols) {
                let r = n.div_ceil(c);
                if r > max_rows {
                    continue;
                }
                let ratio = (c as f64) / (r as f64);
                let score = (ratio - aspect).abs();
                if score < best_score {
                    best_score = score;
                    best_cols = c;
                }
            }

            // Verify actual row count via flow layout (simple div_ceil can
            // undercount when MiniMax forces a row wrap mid-row).
            let actual_rows = flow_rows(best_cols);
            let cols = if actual_rows <= max_rows {
                best_cols
            } else {
                // Try wider layouts; a higher col count may let MiniMax fit
                // alongside its neighbour instead of wrapping to its own row.
                let wider = (best_cols..=max_cols).find(|&c| flow_rows(c) <= max_rows);
                match wider {
                    Some(c) => c,
                    // Nothing fits on one page — paginate with a corrected per_page.
                    // max_cols * max_rows over-counts when MiniMax causes extra rows;
                    // find the actual N that fits.
                    None => {
                        let pp = find_fitting_per_page(max_cols);
                        return GridLayout {
                            cols: max_cols,
                            per_page: pp,
                        };
                    }
                }
            };
            GridLayout { cols, per_page: n }
        } else {
            // Paginate: use max_cols with a flow-correct per_page.
            let pp = find_fitting_per_page(max_cols);
            GridLayout {
                cols: max_cols,
                per_page: pp,
            }
        }
    }

    fn render_grid(&self, f: &mut Frame) {
        let area = f.area();

        if self.entries.is_empty() {
            let paragraph = Paragraph::new(
                "No providers configured. Set API keys via environment variables or config files.",
            )
            .block(Block::new().borders(Borders::ALL).title("quotas"));
            f.render_widget(paragraph, area);
            return;
        }

        let title_h = 2;
        let footer_h = 1;
        let title_area = Rect::new(area.x, area.y, area.width, title_h);
        let grid_area = Rect::new(
            area.x,
            area.y + title_h,
            area.width,
            area.height.saturating_sub(title_h + footer_h),
        );
        let footer_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(footer_h),
            area.width,
            footer_h,
        );

        let layout = self.compute_layout(grid_area);
        self.last_layout.set(layout);

        // Visible = non-AuthRequired providers shown in the grid.
        let total = self.visible_count();
        let page = self.current_page();
        let pages = total.div_ceil(layout.per_page.max(1));
        let page_start = page * layout.per_page;
        let page_end = (page_start + layout.per_page).min(total);

        // Count in-flight fetches (Loading = first-ever load; Refreshing = refresh).
        let loading_count = self
            .entries
            .iter()
            .filter(|e| matches!(e, ProviderEntry::Loading | ProviderEntry::Refreshing(_)))
            .count();

        // Collect names of providers with no API key for the footer indicator.
        let unconfigured: Vec<&str> = self
            .entries
            .iter()
            .filter_map(|e| match e {
                ProviderEntry::Done(r) | ProviderEntry::Refreshing(r)
                    if matches!(r.status, ProviderStatus::AuthRequired) =>
                {
                    Some(r.kind.display_name())
                }
                _ => None,
            })
            .collect();

        let mut header_line = vec![Span::raw(" quotas ").bold().white()];
        if loading_count > 0 {
            header_line.push(Span::raw("  "));
            header_line.push(
                Span::raw(format!(
                    "{} fetching {} provider{}…",
                    SPINNER[self.spinner_frame],
                    loading_count,
                    if loading_count == 1 { "" } else { "s" }
                ))
                .cyan(),
            );
        }
        if pages > 1 {
            header_line.push(Span::raw("   "));
            header_line.push(Span::raw(format!("page {}/{}", page + 1, pages)).dim());
        }

        // Build the hint line with tracked button positions for mouse clicks/hover.
        // Char offsets (prefix = 25, R Refresh = 9, middle = 26, Q Quit = 6):
        //   " ←↑↓→ Nav  Enter Detail  " (25) + "R Refresh" + "  C Copy  PgUp/PgDn Page  " (26) + "Q Quit"
        let hint_y = title_area.y + 1;
        let mouse = self.mouse_pos.get();
        let refresh_rect = Rect::new(title_area.x + 25, hint_y, 9, 1);
        let quit_rect = Rect::new(title_area.x + 60, hint_y, 6, 1);
        self.refresh_btn.set(Some(refresh_rect));
        self.quit_btn.set(Some(quit_rect));

        let in_rect = |r: Rect| -> bool {
            mouse.is_some_and(|(c, row)| {
                c >= r.x && c < r.x + r.width && row == r.y
            })
        };
        let btn_style = |r: Rect| -> Style {
            if in_rect(r) {
                Style::new().white().bold().underlined()
            } else {
                Style::new().dim()
            }
        };

        let hint_line = Line::from(vec![
            Span::raw(" ←↑↓→ Nav  Enter Detail  ").dim(),
            Span::styled("R Refresh", btn_style(refresh_rect)),
            Span::raw("  C Copy  PgUp/PgDn Page  ").dim(),
            Span::styled("Q Quit", btn_style(quit_rect)),
            Span::raw("  · ").dim(),
            Span::raw("▒").fg(Color::DarkGray),
            Span::raw(" ahead  ").dim(),
            Span::raw("█").fg(Color::Rgb(255, 140, 0)),
            Span::raw(" overspend ").dim(),
        ]);

        let title = Paragraph::new(vec![Line::from(header_line), hint_line])
            .block(Block::new().borders(Borders::NONE));
        f.render_widget(title, title_area);

        let loaded = total.saturating_sub(loading_count);
        let footer_text = if unconfigured.is_empty() {
            format!("{} of {} providers loaded", loaded, total)
        } else {
            format!(
                "{} of {} loaded  ·  No key: {}",
                loaded,
                total,
                unconfigured.join(", ")
            )
        };
        let footer = Paragraph::new(footer_text).style(Style::new().dim());
        f.render_widget(footer, footer_area);

        let order = self.visual_order();
        let page_count = page_end - page_start;
        let cols = layout.cols.min(page_count.max(1));

        // Flow-based placement: MiniMax spans 2 columns, all others span 1.
        // This keeps card widths at a consistent unit_col_w so MiniMax is
        // always exactly twice as wide as its neighbours.
        let placements = Self::flow_placements(&self.entries, &order[page_start..page_end], cols);
        let num_rows = placements
            .iter()
            .map(|(r, _, _)| *r + 1)
            .max()
            .unwrap_or(0)
            .max(1);

        // Each row gets the tallest natural card height in that row.
        let mut row_heights: Vec<u16> = vec![MIN_CARD_H; num_rows];
        for (i, visual_pos) in (page_start..page_end).enumerate() {
            let (row_i, _, _) = placements[i];
            let entry_idx = order[visual_pos];
            let h = Self::natural_card_height(&self.entries[entry_idx]);
            if h > row_heights[row_i] {
                row_heights[row_i] = h;
            }
        }

        // When natural heights overflow the grid, compress from the tallest rows
        // downward (down to MIN_CARD_H) rather than scaling everything
        // proportionally.  Preserving short rows means providers with few
        // windows keep TwoLine mode (reset lines visible) while taller rows
        // with many windows gracefully fall back to OneLine.
        let total_fixed: u16 = row_heights.iter().sum();
        if total_fixed > grid_area.height {
            // Sort indices largest-first so we shave from the biggest rows.
            let mut indices: Vec<usize> = (0..row_heights.len()).collect();
            indices.sort_by(|&a, &b| row_heights[b].cmp(&row_heights[a]));
            let mut remaining = total_fixed.saturating_sub(grid_area.height);
            for &idx in &indices {
                if remaining == 0 {
                    break;
                }
                let can_shrink = row_heights[idx].saturating_sub(MIN_CARD_H);
                let shrink = remaining.min(can_shrink);
                row_heights[idx] -= shrink;
                remaining -= shrink;
            }
        }
        let mut row_constraints: Vec<Constraint> =
            row_heights.iter().map(|h| Constraint::Length(*h)).collect();
        row_constraints.push(Constraint::Fill(1));
        let all_rects = Layout::vertical(row_constraints).split(grid_area);
        let row_rects = &all_rects[..num_rows];

        // Unit column width: all cards use multiples of this so columns align.
        let unit_col_w = (grid_area.width / cols as u16).max(1);

        let mut hit_areas: Vec<(Rect, usize)> = Vec::with_capacity(page_end - page_start);
        for (i, visual_pos) in (page_start..page_end).enumerate() {
            let (row_i, col_i, span) = placements[i];
            let row_area = row_rects[row_i];
            let x = row_area.x + col_i as u16 * unit_col_w;
            let w = (span as u16 * unit_col_w).saturating_sub(1);
            let card_area = Rect::new(x, row_area.y, w, row_area.height);
            let selected = visual_pos == self.selected_index;
            let entry_idx = order[visual_pos];
            hit_areas.push((card_area, visual_pos));
            self.render_entry(f, entry_idx, selected, card_area);
        }
        *self.card_hit_areas.borrow_mut() = hit_areas;
    }

    /// Natural height of a card in rows including the border (2 lines).
    /// Used so row heights track content rather than filling all available space.
    fn natural_card_height(entry: &ProviderEntry) -> u16 {
        match entry {
            ProviderEntry::Loading => MIN_CARD_H,
            ProviderEntry::Done(r) | ProviderEntry::Refreshing(r) => match &r.status {
                ProviderStatus::Available { quota } => {
                    let visible = quota
                        .windows
                        .iter()
                        .filter(|w| w.limit > 0 || bar::currency_window(&w.window_type).is_some())
                        .count();
                    // 2 border + 1 header (name+freshness) + 1 plan name
                    let fixed: u16 = 4;
                    // +1 for footer_reserve that render_done_card always subtracts.
                    let content: u16 = if r.kind == ProviderKind::Minimax {
                        // 2-col render: 1 col-header row + 1 row per model pair
                        // + 1 reset-period footer row per unique period.
                        let model_rows = visible.div_ceil(2) as u16;
                        1 + model_rows + 2 // 2 reset lines (one per period)
                    } else {
                        // TwoLine: 2 lines per window (bar + reset).
                        (visible as u16) * 2 + 1
                    };
                    (fixed + content).max(MIN_CARD_H)
                }
                // Auth-required cards are squat indicator boxes: just border +
                // header line + one status line.
                ProviderStatus::AuthRequired => 4,
                _ => MIN_CARD_H,
            },
        }
    }

    fn card_weight(entry: &ProviderEntry) -> u32 {
        match entry {
            ProviderEntry::Loading => 4,
            ProviderEntry::Done(r) | ProviderEntry::Refreshing(r) => match &r.status {
                ProviderStatus::Available { quota } => {
                    let visible = quota
                        .windows
                        .iter()
                        .filter(|w| w.limit > 0 || bar::currency_window(&w.window_type).is_some())
                        .count()
                        .max(1) as u32;
                    // Minimax renders 5h/7d pairs on one line, so its
                    // vertical footprint is ~half the window count.
                    let effective = if r.kind == ProviderKind::Minimax {
                        visible.div_ceil(2)
                    } else {
                        visible
                    };
                    // Score grows with content but is capped so a card
                    // with 30 windows can't monopolize an entire row at
                    // the expense of the others.
                    let raw = 4 + effective;
                    raw.clamp(5, 10)
                }
                // Push auth-required cards to the end of the visual order.
                ProviderStatus::AuthRequired => 20,
                _ => 5,
            },
        }
    }

    fn render_entry(&self, f: &mut Frame, idx: usize, selected: bool, area: Rect) {
        let border_style = if selected {
            Style::new().green()
        } else {
            Style::new().dim()
        };
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(border_style);
        let raw_inner = block.inner(area);
        f.render_widget(block, area);

        // Add 1sp left padding so content doesn't press against the left border.
        let inner = Rect::new(
            raw_inner.x + 1,
            raw_inner.y,
            raw_inner.width.saturating_sub(1),
            raw_inner.height,
        );

        match &self.entries[idx] {
            ProviderEntry::Loading => {
                let name = self.kinds[idx].display_name();
                let spinner_char = SPINNER[self.spinner_frame];
                let text = Text::from(vec![
                    Line::from(vec![Span::raw(format!(
                        "{} {}",
                        if selected { "▶" } else { " " },
                        name
                    ))
                    .bold()]),
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(spinner_char).cyan(),
                        Span::raw(" loading…").dim(),
                    ]),
                ]);
                let p = Paragraph::new(text);
                f.render_widget(p, inner);
            }
            ProviderEntry::Refreshing(result) => {
                // Show the stale data so the user retains context, then
                // overlay a small spinner in the top-right of the header row.
                self.render_done_card(f, result, selected, inner);
                let spin = format!("{} ", SPINNER[self.spinner_frame]);
                let spin_w = spin.chars().count() as u16;
                if inner.width >= spin_w + 2 {
                    let x = inner.x + inner.width - spin_w;
                    f.render_widget(
                        Paragraph::new(Line::from(Span::raw(spin).cyan().dim())),
                        Rect::new(x, inner.y, spin_w, 1),
                    );
                }
            }
            ProviderEntry::Done(result) => {
                self.render_done_card(f, result, selected, inner);
            }
        }
    }

    fn render_done_card(
        &self,
        f: &mut Frame,
        result: &ProviderResult,
        selected: bool,
        inner: Rect,
    ) {
        use crate::tui::freshness::{FreshnessLabel, Staleness};

        let is_auth = matches!(result.status, ProviderStatus::AuthRequired);

        let mut lines: Vec<Line> = Vec::new();

        // Header line: name (+ freshness progress bar when auth is present).
        let name_part = format!(
            "{} {}",
            if selected { "▶" } else { " " },
            result.kind.display_name()
        );
        let name_len = name_part.chars().count();
        let avail_for_fresh = (inner.width as usize).saturating_sub(name_len + 2);

        let header_line = if is_auth || avail_for_fresh == 0 {
            // Auth-required or no space: just the name, slightly dimmed for auth.
            let style = if is_auth {
                Style::new().bold().dim()
            } else {
                Style::new().bold()
            };
            Line::from(Span::styled(
                bar::truncate_suffix(&name_part, inner.width as usize),
                style,
            ))
        } else {
            // Freshness progress bar behind the label text.
            let elapsed = (chrono::Utc::now() - result.fetched_at)
                .num_seconds()
                .max(0);
            let freshness = FreshnessLabel::with_interval(elapsed, result.kind.auto_refresh_secs());
            let fresh_str = &freshness.label;

            let field_w = fresh_str.chars().count().min(avail_for_fresh);
            // Pad/truncate to exactly field_w chars.
            let text: String = fresh_str.chars().take(field_w).collect();
            let text = format!("{:<width$}", text, width = field_w);
            let text: String = text.chars().take(field_w).collect();

            let fill_n = (freshness.fraction * field_w as f64).round() as usize;
            let fill_n = fill_n.min(field_w);

            let (fg, bg_fill) = match freshness.staleness {
                Staleness::Fresh => (Color::Cyan, Color::Rgb(0, 42, 28)),
                Staleness::Warning => (Color::Yellow, Color::Rgb(65, 38, 0)),
                Staleness::Stale => (Color::Red, Color::Rgb(65, 0, 0)),
            };

            let filled_str: String = text.chars().take(fill_n).collect();
            let unfilled_str: String = text.chars().skip(fill_n).collect();

            let mut spans: Vec<Span> = vec![Span::raw(name_part).bold(), Span::raw("  ")];
            if !filled_str.is_empty() {
                spans.push(Span::styled(filled_str, Style::new().fg(fg).bg(bg_fill)));
            }
            if !unfilled_str.is_empty() {
                spans.push(Span::styled(unfilled_str, Style::new().fg(fg).dim()));
            }
            Line::from(spans)
        };
        lines.push(header_line);

        // Plan / status line
        match &result.status {
            ProviderStatus::Available { quota } => {
                lines.push(Line::from(
                    Span::raw(quota.plan_name.clone()).italic().dim(),
                ));

                if result.kind == ProviderKind::Minimax {
                    render_minimax_windows(&mut lines, &quota.windows, inner.width);
                    let paragraph = Paragraph::new(Text::from(lines))
                        .alignment(ratatui::layout::Alignment::Left);
                    f.render_widget(paragraph, inner);
                    return;
                }

                let mut visible: Vec<&QuotaWindow> = quota
                    .windows
                    .iter()
                    .filter(|w| w.limit > 0 || bar::currency_window(&w.window_type).is_some())
                    .collect();
                visible.sort_by_key(|w| bar::window_sort_key(w));
                let total = visible.len();

                // Count distinct period buckets — used to decide whether
                // to emit section headers (only useful with ≥2 buckets).
                let mut buckets_seen = std::collections::BTreeSet::new();
                for w in &visible {
                    buckets_seen.insert(bar::window_sort_key(w).0);
                }
                let show_headers = total >= 6 && buckets_seen.len() >= 2;

                let header_lines = lines.len() as u16;
                let body_height = inner.height.saturating_sub(header_lines);
                let footer_reserve: u16 = if total > 0 { 1 } else { 0 };
                let usable = body_height.saturating_sub(footer_reserve) as usize;

                // Headers eat one line each; subtract from usable when used.
                let header_budget = if show_headers { buckets_seen.len() } else { 0 };
                let usable_for_rows = usable.saturating_sub(header_budget);

                let (mode, shown_count) = pick_layout(total, usable_for_rows);

                let label_w: usize = 12;
                let bar_width = inner.width.saturating_sub(label_w as u16 + 2).clamp(10, 64);

                let mut last_bucket: Option<u8> = None;
                for w in visible.iter().take(shown_count) {
                    let bucket = bar::window_sort_key(w).0;
                    if show_headers && Some(bucket) != last_bucket {
                        if let Some(label) = bar::bucket_label(bucket) {
                            lines.push(Line::from(Span::raw(label.to_string()).dim()));
                        }
                        last_bucket = Some(bucket);
                    }

                    let label_src = bar::display_label(&w.window_type, show_headers);
                    if let Some((sym, scale)) = bar::currency_window(&w.window_type) {
                        lines.push(Line::from(vec![Span::raw(format!(
                            "{:<width$} {}{:.2}",
                            label_src,
                            sym,
                            w.remaining as f64 / scale,
                            width = label_w
                        ))]));
                        if matches!(mode, RenderMode::TwoLine) {
                            lines.push(Line::from(""));
                        }
                        continue;
                    }
                    let used_pct = (w.used as f64 / w.limit.max(1) as f64).clamp(0.0, 1.0);
                    let time_elapsed = bar::time_elapsed_fraction(w);
                    let color = bar::bar_color(used_pct);
                    let overlay = bar_overlay_text(used_pct, w.used, w.limit, bar_width as usize);
                    let bar_spans =
                        bar::build_labeled(used_pct, time_elapsed, bar_width, color, &overlay);

                    let mut l1 = vec![Span::raw(format!(
                        "{:<width$} ",
                        bar::truncate_suffix(&label_src, label_w),
                        width = label_w
                    ))];
                    l1.extend(bar_spans);
                    lines.push(Line::from(l1));

                    if matches!(mode, RenderMode::TwoLine) {
                        if let Some(reset_at) = w.reset_at {
                            let rel = humanize_reset(reset_at - chrono::Utc::now());
                            lines.push(Line::from(
                                Span::raw(format!(
                                    "{:width$}resets in {}",
                                    "",
                                    rel,
                                    width = label_w + 1
                                ))
                                .dim(),
                            ));
                        }
                    }
                }

                if total > shown_count {
                    lines.push(Line::from(
                        Span::raw(format!("+ {} more · Enter for detail", total - shown_count))
                            .dim()
                            .italic(),
                    ));
                } else if matches!(mode, RenderMode::TwoLine) {
                    // All windows fit in TwoLine mode — there may be spare rows
                    // (because row height = max across cards in the row). Use
                    // one of them for a compact pacing badge.
                    if let Some((badge_text, badge_style)) = pace_badge(&visible) {
                        lines.push(Line::from(""));
                        lines.push(Line::from(vec![
                            Span::raw(" "),
                            Span::styled(badge_text, badge_style),
                        ]));
                    }
                }
            }
            ProviderStatus::Unavailable { info } => {
                lines.push(Line::from(
                    Span::raw(format!("Unavailable: {}", info.reason)).yellow(),
                ));
                if let Some(url) = &info.console_url {
                    lines.push(Line::from(Span::raw(url.clone()).dim()));
                }
            }
            ProviderStatus::AuthRequired => {
                lines.push(Line::from(Span::raw("Set API key in env or config").dim()));
            }
            ProviderStatus::NetworkError { message } => {
                lines.push(Line::from(Span::raw("Network error").red()));
                lines.push(Line::from(Span::raw(message.clone()).dim()));
            }
        }

        // No wrap — lines clip at card width rather than breaking the layout.
        let paragraph =
            Paragraph::new(Text::from(lines)).alignment(ratatui::layout::Alignment::Left);
        f.render_widget(paragraph, inner);
    }

    pub fn page_up(&mut self) {
        let layout = self.last_layout.get();
        if layout.per_page == 0 {
            return;
        }
        let page = self.current_page();
        if page > 0 {
            let new_page = page - 1;
            self.selected_index = new_page * layout.per_page;
        }
    }

    pub fn page_down(&mut self) {
        let layout = self.last_layout.get();
        if layout.per_page == 0 {
            return;
        }
        let total = self.visible_count();
        let page = self.current_page();
        let pages = total.div_ceil(layout.per_page);
        if page + 1 < pages {
            self.selected_index = (page + 1) * layout.per_page;
        }
    }

    fn render_detail(&self, f: &mut Frame) {
        let area = f.area();

        let title = Paragraph::new(vec![
            Line::from(vec![Span::raw(" QUOTA DETAIL ").bold().white()]),
            Line::from(vec![Span::raw(
                " ← → providers  ↑ ↓ scroll  Enter/Esc back  C copy  Q quit ",
            )
            .dim()]),
        ])
        .block(Block::new().borders(Borders::BOTTOM));
        f.render_widget(title, Rect::new(area.x, area.y, area.width, 3));

        if let Some(selected) = self.selected_provider() {
            let view = DetailView::new(selected.clone());
            let detail_area = Rect::new(
                area.x,
                area.y + 3,
                area.width,
                area.height.saturating_sub(4),
            );
            let text = view.render(detail_area.width);
            let paragraph = Paragraph::new(text)
                .block(Block::new().borders(Borders::NONE))
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll, 0));
            f.render_widget(paragraph, detail_area);
        } else {
            let p = Paragraph::new("Provider still loading…")
                .block(Block::new().borders(Borders::NONE));
            f.render_widget(
                p,
                Rect::new(
                    area.x,
                    area.y + 3,
                    area.width,
                    area.height.saturating_sub(4),
                ),
            );
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum RenderMode {
    /// Two lines per window: bar+pct, then exact `used / limit left`.
    TwoLine,
    /// One line per window: label, bar, pct, remaining count.
    OneLine,
}

/// Pick the densest rendering that fits the most windows in the available
/// vertical space. Returns the chosen mode and the number of windows to
/// actually render — anything beyond that is summarized as "+ N more".
fn pick_layout(total: usize, usable_lines: usize) -> (RenderMode, usize) {
    if total == 0 || usable_lines == 0 {
        return (RenderMode::TwoLine, 0);
    }
    // Try the comfortable two-line layout first.
    let two_fits = usable_lines / 2;
    if two_fits >= total {
        return (RenderMode::TwoLine, total);
    }
    // Otherwise switch to compact one-line format.
    let one_fits = usable_lines;
    if one_fits >= total {
        return (RenderMode::OneLine, total);
    }
    // Still doesn't fit — show as many as possible compactly and reserve
    // the last line for the "+ N more" indicator (the caller already
    // subtracted that line from usable_lines).
    (RenderMode::OneLine, one_fits.min(total))
}

/// MiniMax-specific card body: pair each model's 5h and 7d windows onto
/// a single row `label | 5h bar | 7d bar`, so both periods are visible
/// at once and the full-width card isn't mostly dead space.
fn render_minimax_windows(lines: &mut Vec<Line<'_>>, windows: &[QuotaWindow], inner_w: u16) {
    // Preserve input order of first sighting so the provider's ranking
    // (coding plans first, then everything else) survives.
    let mut order: Vec<String> = Vec::new();
    let mut pairs: std::collections::HashMap<String, (Option<&QuotaWindow>, Option<&QuotaWindow>)> =
        std::collections::HashMap::new();

    for w in windows {
        let (model, is_five) = if let Some(rest) = w.window_type.strip_prefix("5h/") {
            (rest.to_string(), true)
        } else if let Some(rest) = w.window_type.strip_prefix("wk/") {
            (rest.to_string(), false)
        } else if let Some(rest) = w.window_type.strip_prefix("7d/") {
            (rest.to_string(), false)
        } else {
            continue;
        };
        if !pairs.contains_key(&model) {
            order.push(model.clone());
        }
        let slot = pairs.entry(model).or_insert((None, None));
        if is_five {
            slot.0 = Some(w);
        } else {
            slot.1 = Some(w);
        }
    }

    if order.is_empty() {
        return;
    }

    // Fit label_w + 2 bars (min 10 each) + spacing into inner_w.
    // Desired label_w is 24; shrink it if the card is narrow.
    // min_needed = label + 1 (space) + bar*2 + 2 (gap) = label + 2*10 + 3 = label + 23
    // → label_max = inner_w.saturating_sub(23)
    let label_w: usize = 24usize.min(inner_w.saturating_sub(23) as usize).max(8);
    // Reserve: label + trailing space + gap between bars (2 chars).
    let reserved = label_w as u16 + 1 + 2;
    let avail = inner_w.saturating_sub(reserved);
    let bar_w: u16 = (avail / 2).clamp(10, 90);

    // Column header row.
    let header_5h = format!("{:^w$}", "── 5h ──", w = bar_w as usize);
    let header_7d = format!("{:^w$}", "── 7d ──", w = bar_w as usize);
    lines.push(Line::from(vec![
        Span::raw(format!("{:<w$} ", "", w = label_w)),
        Span::raw(header_5h).dim(),
        Span::raw("  "),
        Span::raw(header_7d).dim(),
    ]));

    // Collect reset times for the two period columns while rendering.
    let mut reset_5h: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut reset_7d: Option<chrono::DateTime<chrono::Utc>> = None;

    for model in &order {
        let (five, seven) = pairs.get(model).copied().unwrap_or((None, None));
        let label = bar::truncate_suffix(model, label_w);
        let mut spans: Vec<Span<'_>> = vec![Span::raw(format!("{:<w$} ", label, w = label_w))];

        spans.extend(minimax_bar_cell(five, bar_w));
        spans.push(Span::raw("  "));
        spans.extend(minimax_bar_cell(seven, bar_w));

        lines.push(Line::from(spans));

        if reset_5h.is_none() {
            if let Some(w) = five {
                reset_5h = w.reset_at;
            }
        }
        if reset_7d.is_none() {
            if let Some(w) = seven {
                reset_7d = w.reset_at;
            }
        }
    }

    // Footer: show reset times for each period, grouped (all models share
    // the same window boundary for a given period).
    let pad = format!("{:<w$} ", "", w = label_w);
    if reset_5h.is_some() || reset_7d.is_some() {
        let mut footer_spans: Vec<Span<'_>> = vec![Span::raw(pad.clone())];
        if let Some(t) = reset_5h {
            let rel = humanize_reset(t - chrono::Utc::now());
            footer_spans.push(
                Span::raw(format!(
                    "{:^w$}",
                    format!("5h resets in {}", rel),
                    w = bar_w as usize
                ))
                .dim(),
            );
        } else {
            footer_spans.push(Span::raw(format!("{:w$}", "", w = bar_w as usize)));
        }
        footer_spans.push(Span::raw("  "));
        if let Some(t) = reset_7d {
            let rel = humanize_reset(t - chrono::Utc::now());
            footer_spans.push(
                Span::raw(format!(
                    "{:^w$}",
                    format!("7d resets in {}", rel),
                    w = bar_w as usize
                ))
                .dim(),
            );
        }
        lines.push(Line::from(footer_spans));
    }
}

fn minimax_bar_cell(win: Option<&QuotaWindow>, bar_w: u16) -> Vec<Span<'static>> {
    match win {
        Some(w) if w.limit > 0 => {
            let used_pct = (w.used as f64 / w.limit.max(1) as f64).clamp(0.0, 1.0);
            let time_elapsed = bar::time_elapsed_fraction(w);
            let color = bar::bar_color(used_pct);
            let overlay = bar_overlay_text(used_pct, w.used, w.limit, bar_w as usize);
            bar::build_labeled(used_pct, time_elapsed, bar_w, color, &overlay)
        }
        _ => vec![Span::raw(format!("{:w$}", "", w = bar_w as usize))],
    }
}

/// Returns a compact pacing summary badge for the bottom of a card when
/// all windows are visible in TwoLine mode and spare rows exist.
/// Returns None when the pace is neutral enough that a badge adds no signal.
fn pace_badge(visible: &[&QuotaWindow]) -> Option<(String, Style)> {
    let mut worst_diff: f64 = f64::NEG_INFINITY;
    let mut worst_label = String::new();
    let mut worst_pct: f64 = 0.0;
    for w in visible {
        if bar::currency_window(&w.window_type).is_some() {
            continue;
        }
        let used_pct = (w.used as f64 / w.limit.max(1) as f64).clamp(0.0, 1.0);
        let Some(elapsed) = bar::time_elapsed_fraction(w) else {
            continue;
        };
        let diff = used_pct - elapsed;
        if diff > worst_diff {
            worst_diff = diff;
            worst_label = bar::display_label(&w.window_type, false);
            worst_pct = used_pct;
        }
    }
    if worst_diff == f64::NEG_INFINITY {
        return None;
    }
    if worst_diff >= 0.08 {
        let text = format!(
            "⚡ {}: {:.0}% — burning fast",
            bar::truncate_suffix(&worst_label, 10),
            worst_pct * 100.0
        );
        Some((text, Style::new().fg(Color::Rgb(255, 140, 0))))
    } else if worst_diff <= -0.08 {
        Some(("✓ all pacing ahead".to_string(), Style::new().green().dim()))
    } else {
        None // neutral — don't clutter the card
    }
}

fn bar_overlay_text(used_pct: f64, used: i64, limit: i64, bar_width: usize) -> String {
    let pct = format!("{:.0}%", used_pct * 100.0);
    if bar_width < 10 {
        return pct;
    }
    let nums = format!("{}/{}", format_num(used), format_num(limit));
    let compact = format!("{} {}", pct, nums);
    if compact.chars().count() + 2 <= bar_width {
        format!("{} ({})", pct, nums)
    } else if compact.chars().count() <= bar_width {
        compact
    } else {
        pct
    }
}

fn humanize_reset(d: chrono::Duration) -> String {
    let secs = d.num_seconds();
    if secs <= 0 {
        return "now".to_string();
    }
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins.max(1))
    }
}

fn format_num(n: i64) -> String {
    fn trim_trailing_zero(s: String) -> String {
        if let Some(stripped) = s.strip_suffix(".0") {
            stripped.to_string()
        } else {
            s
        }
    }
    if n.abs() >= 1_000_000 {
        trim_trailing_zero(format!("{:.1}", n as f64 / 1_000_000.0)) + "M"
    } else if n.abs() >= 1_000 {
        trim_trailing_zero(format!("{:.1}", n as f64 / 1_000.0)) + "k"
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_num_scales() {
        assert_eq!(format_num(500), "500");
        assert_eq!(format_num(1_500), "1.5k");
        assert_eq!(format_num(2_000), "2k");
        assert_eq!(format_num(150_000), "150k");
        assert_eq!(format_num(2_500_000), "2.5M");
    }
}
