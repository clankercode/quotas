use super::bar;
use super::detail::DetailView;
use super::provider_card::ProviderCard;
use crate::providers::{ProviderKind, ProviderResult, ProviderStatus, QuotaWindow};
use ratatui::layout::Rect;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::cell::Cell;

#[derive(Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone)]
pub enum ProviderEntry {
    Loading,
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
    pub spinner_frame: usize,
    last_layout: Cell<GridLayout>,
}

impl Dashboard {
    pub fn new_loading(kinds: Vec<ProviderKind>) -> Self {
        let entries = kinds.iter().map(|_| ProviderEntry::Loading).collect();
        Self {
            kinds,
            entries,
            selected_index: 0,
            show_detail: false,
            spinner_frame: 0,
            last_layout: Cell::new(GridLayout::default()),
        }
    }

    pub fn update(&mut self, idx: usize, result: ProviderResult) {
        if idx < self.entries.len() {
            self.entries[idx] = ProviderEntry::Done(result);
        }
    }

    pub fn reset_loading(&mut self) {
        for e in &mut self.entries {
            *e = ProviderEntry::Loading;
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
            ProviderEntry::Done(r) => Some(r),
            ProviderEntry::Loading => None,
        }
    }

    /// Permutation of entry indices sorted by `card_weight` ascending so
    /// short cards render at the top and the tallest (usually minimax)
    /// ends up in its own row at the bottom. Stable so loaded cards don't
    /// jitter relative to each other as long as their weights tie.
    fn visual_order(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.entries.len()).collect();
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
        let total = self.entries.len();
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

    fn compute_layout(&self, grid_area: Rect) -> GridLayout {
        let n = self.entries.len().max(1);
        let max_cols = (grid_area.width / MIN_CARD_W).max(1) as usize;
        let max_rows = (grid_area.height / MIN_CARD_H).max(1) as usize;
        let per_page = (max_cols * max_rows).max(1);

        if n <= per_page {
            // Choose cols balancing terminal aspect
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
            let cols = best_cols;
            GridLayout { cols, per_page: n }
        } else {
            // Paginate: use max_cols × max_rows
            GridLayout {
                cols: max_cols,
                per_page,
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

        let total = self.entries.len();
        let page = self.current_page();
        let pages = total.div_ceil(layout.per_page.max(1));
        let page_start = page * layout.per_page;
        let page_end = (page_start + layout.per_page).min(total);

        let loading_count = self
            .entries
            .iter()
            .filter(|e| matches!(e, ProviderEntry::Loading))
            .count();

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
        let title = Paragraph::new(vec![
            Line::from(header_line),
            Line::from(vec![
                Span::raw(" ←↑↓→ Nav  Enter Detail  R Refresh  C Copy  PgUp/PgDn Page  Q Quit  · ")
                    .dim(),
                Span::raw("▒").fg(Color::DarkGray),
                Span::raw(" ahead  ").dim(),
                Span::raw("█").fg(Color::Rgb(255, 140, 0)),
                Span::raw(" overspend ").dim(),
            ]),
        ])
        .block(Block::new().borders(Borders::NONE));
        f.render_widget(title, title_area);

        let footer = Paragraph::new(format!(
            "{} of {} providers loaded",
            total - loading_count,
            total
        ))
        .style(Style::new().dim());
        f.render_widget(footer, footer_area);

        let order = self.visual_order();
        let page_count = page_end - page_start;
        let cols = layout.cols.min(page_count.max(1));
        let rows = page_count.div_ceil(cols);

        // Row heights are weighted by the max content weight of any card in
        // that row, so a row containing a minimax card with 10 windows gets
        // more vertical space than a row of single-window zai/claude cards.
        let mut row_weights: Vec<u32> = vec![1; rows];
        for (i, visual_pos) in (page_start..page_end).enumerate() {
            let row_i = i / cols;
            let entry_idx = order[visual_pos];
            let w = Self::card_weight(&self.entries[entry_idx]);
            if w > row_weights[row_i] {
                row_weights[row_i] = w;
            }
        }
        let total_weight: u32 = row_weights.iter().sum::<u32>().max(1);
        let row_constraints: Vec<Constraint> = row_weights
            .iter()
            .map(|w| Constraint::Ratio(*w, total_weight))
            .collect();
        let row_rects = Layout::vertical(row_constraints).split(grid_area);

        for (i, visual_pos) in (page_start..page_end).enumerate() {
            let col_i = i % cols;
            let row_i = i / cols;
            let row_area = row_rects[row_i];
            // Cards actually laid out in this row (partial last row must
            // stretch its remaining cards across the full row width).
            let row_start = row_i * cols;
            let row_end = (row_start + cols).min(page_count);
            let cards_in_row = (row_end - row_start).max(1);
            let col_w = row_area.width / cards_in_row as u16;
            let x = row_area.x + col_i as u16 * col_w;
            let card_area = Rect::new(x, row_area.y, col_w.saturating_sub(1), row_area.height);
            let selected = visual_pos == self.selected_index;
            let entry_idx = order[visual_pos];
            self.render_entry(f, entry_idx, selected, card_area);
        }
    }

    fn card_weight(entry: &ProviderEntry) -> u32 {
        match entry {
            ProviderEntry::Loading => 4,
            ProviderEntry::Done(r) => match &r.status {
                ProviderStatus::Available { quota } => {
                    let visible = quota
                        .windows
                        .iter()
                        .filter(|w| w.limit > 0 || w.window_type == "payg_balance")
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
        let inner = block.inner(area);
        f.render_widget(block, area);

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
                let p = Paragraph::new(text).wrap(Wrap { trim: true });
                f.render_widget(p, inner);
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
        let card = ProviderCard::new(result.clone(), selected);
        let freshness = card.freshness_label();
        let freshness_style = match freshness.staleness {
            crate::tui::freshness::Staleness::Fresh => Style::new().cyan(),
            crate::tui::freshness::Staleness::Warning => Style::new().yellow(),
            crate::tui::freshness::Staleness::Stale => Style::new().red(),
        };

        let mut lines: Vec<Line> = Vec::new();

        // Header line: name + freshness
        lines.push(Line::from(vec![
            Span::raw(format!(
                "{} {}",
                if selected { "▶" } else { " " },
                card.display_name()
            ))
            .bold(),
            Span::raw("  "),
            Span::styled(freshness.label.clone(), freshness_style),
        ]));

        // Plan / status line
        match &result.status {
            ProviderStatus::Available { quota } => {
                lines.push(Line::from(
                    Span::raw(quota.plan_name.clone()).italic().dim(),
                ));

                if result.kind == ProviderKind::Minimax {
                    render_minimax_windows(&mut lines, &quota.windows, inner.width);
                    let paragraph = Paragraph::new(Text::from(lines))
                        .wrap(Wrap { trim: false })
                        .alignment(ratatui::layout::Alignment::Left);
                    f.render_widget(paragraph, inner);
                    return;
                }

                let mut visible: Vec<&QuotaWindow> = quota
                    .windows
                    .iter()
                    .filter(|w| w.limit > 0 || w.window_type == "payg_balance")
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
                    if w.window_type == "payg_balance" {
                        lines.push(Line::from(vec![Span::raw(format!(
                            "{:<width$} ${:.2}",
                            label_src,
                            w.remaining as f64 / 100.0,
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
                lines.push(Line::from(Span::raw("Auth required").red()));
                lines.push(Line::from(Span::raw("Set API key in env or config").dim()));
            }
            ProviderStatus::NetworkError { message } => {
                lines.push(Line::from(Span::raw("Network error").red()));
                lines.push(Line::from(Span::raw(message.clone()).dim()));
            }
        }

        let paragraph = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .alignment(ratatui::layout::Alignment::Left);
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
        let total = self.entries.len();
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
            Line::from(vec![Span::raw("Enter: back  C: copy JSON  Q: quit ").dim()]),
        ])
        .block(Block::new().borders(Borders::BOTTOM));
        f.render_widget(title, Rect::new(area.x, area.y, area.width, 2));

        if let Some(selected) = self.selected_provider() {
            let view = DetailView::new(selected.clone());
            let detail_area = Rect::new(
                area.x,
                area.y + 2,
                area.width,
                area.height.saturating_sub(3),
            );
            let text = view.render(detail_area.width);
            let paragraph = Paragraph::new(text)
                .block(Block::new().borders(Borders::NONE))
                .wrap(Wrap { trim: false })
                .scroll((0, 0));
            f.render_widget(paragraph, detail_area);
        } else {
            let p = Paragraph::new("Provider still loading…")
                .block(Block::new().borders(Borders::NONE));
            f.render_widget(
                p,
                Rect::new(
                    area.x,
                    area.y + 2,
                    area.width,
                    area.height.saturating_sub(3),
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

    let label_w: usize = 20;
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

    for model in &order {
        let (five, seven) = pairs.get(model).copied().unwrap_or((None, None));
        let label = bar::truncate_suffix(model, label_w);
        let mut spans: Vec<Span<'_>> = vec![Span::raw(format!("{:<w$} ", label, w = label_w))];

        spans.extend(minimax_bar_cell(five, bar_w));
        spans.push(Span::raw("  "));
        spans.extend(minimax_bar_cell(seven, bar_w));

        lines.push(Line::from(spans));
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
