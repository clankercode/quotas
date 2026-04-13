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
        match self.entries.get(self.selected_index)? {
            ProviderEntry::Done(r) => Some(r),
            ProviderEntry::Loading => None,
        }
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
        let area = f.size();

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
            Line::from(vec![Span::raw(
                " ←↑↓→ Navigate  Enter Detail  R Refresh  C Copy  PgUp/PgDn Page  Q Quit ",
            )
            .dim()]),
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

        let page_count = page_end - page_start;
        let cols = layout.cols.min(page_count.max(1));
        let rows = page_count.div_ceil(cols);

        // Row heights are weighted by the max content weight of any card in
        // that row, so a row containing a minimax card with 10 windows gets
        // more vertical space than a row of single-window zai/claude cards.
        let mut row_weights: Vec<u32> = vec![1; rows];
        for (i, entry_idx) in (page_start..page_end).enumerate() {
            let row_i = i / cols;
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

        for (i, entry_idx) in (page_start..page_end).enumerate() {
            let col_i = i % cols;
            let row_i = i / cols;
            let row_area = row_rects[row_i];
            let col_w = row_area.width / cols as u16;
            let x = row_area.x + col_i as u16 * col_w;
            let card_area = Rect::new(x, row_area.y, col_w.saturating_sub(1), row_area.height);
            let selected = entry_idx == self.selected_index;
            self.render_entry(f, entry_idx, selected, card_area);
        }
    }

    fn card_weight(entry: &ProviderEntry) -> u32 {
        match entry {
            ProviderEntry::Loading => 3,
            ProviderEntry::Done(r) => match &r.status {
                ProviderStatus::Available { quota } => {
                    let visible = quota
                        .windows
                        .iter()
                        .filter(|w| w.limit > 0 || w.window_type == "payg_balance")
                        .count()
                        .max(1);
                    // header + plan line + 2 lines per window, clamped
                    // so a single-window card is still readable.
                    (3 + visible as u32 * 2).clamp(5, 32)
                }
                _ => 4,
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
                let bar_width = inner.width.saturating_sub(2).min(40);
                let available_lines = inner.height.saturating_sub(lines.len() as u16);
                let lines_per_window = 2u16;
                let max_windows = (available_lines / lines_per_window) as usize;

                let windows_to_show: Vec<&QuotaWindow> = quota
                    .windows
                    .iter()
                    .filter(|w| w.limit > 0 || w.window_type == "payg_balance")
                    .take(max_windows.max(1))
                    .collect();
                let shown = windows_to_show.len();

                for w in windows_to_show {
                    if w.window_type == "payg_balance" {
                        lines.push(Line::from(vec![Span::raw(format!(
                            "{:<8} ${:.2}",
                            w.window_type,
                            w.remaining as f64 / 100.0
                        ))]));
                        lines.push(Line::from(""));
                        continue;
                    }
                    let used_pct = (w.used as f64 / w.limit.max(1) as f64).clamp(0.0, 1.0);
                    let remaining_pct = 1.0 - used_pct;
                    // Bar fill = what's LEFT, so the visual shrinks as quota
                    // gets burned down. Matches the "N% left" label next to it.
                    let bar = inline_bar(remaining_pct, bar_width);
                    let color = bar_color(remaining_pct);
                    lines.push(Line::from(vec![
                        Span::raw(format!("{:<12} ", truncate(&w.window_type, 12))),
                        Span::styled(bar, Style::new().fg(color)),
                        Span::raw(format!(" {:>3.0}%", remaining_pct * 100.0)),
                    ]));
                    lines.push(Line::from(vec![Span::raw(format!(
                        "             {} / {} left",
                        format_num(w.remaining),
                        format_num(w.limit)
                    ))
                    .dim()]));
                }

                let counted = quota
                    .windows
                    .iter()
                    .filter(|w| w.limit > 0 || w.window_type == "payg_balance")
                    .count();
                if counted > shown {
                    lines.push(Line::from(
                        Span::raw(format!("+ {} more window(s)", counted - shown)).dim(),
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
        let area = f.size();

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

fn inline_bar(pct: f64, width: u16) -> String {
    let w = width as usize;
    if w == 0 {
        return String::new();
    }
    let filled = ((pct.clamp(0.0, 1.0)) * w as f64).round() as usize;
    let empty = w.saturating_sub(filled);
    let mut s = String::with_capacity(w * 3);
    for _ in 0..filled {
        s.push('█');
    }
    for _ in 0..empty {
        s.push('░');
    }
    s
}

fn bar_color(remaining_pct: f64) -> Color {
    // Colors reflect the *remaining* fraction — healthy when lots of quota
    // left, red when almost exhausted.
    if remaining_pct <= 0.10 {
        Color::Red
    } else if remaining_pct <= 0.25 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn format_num(n: i64) -> String {
    if n.abs() >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n.abs() >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_bar_full_and_empty() {
        assert_eq!(inline_bar(0.0, 4), "░░░░");
        assert_eq!(inline_bar(1.0, 4), "████");
        assert_eq!(inline_bar(0.5, 4), "██░░");
    }

    #[test]
    fn format_num_scales() {
        assert_eq!(format_num(500), "500");
        assert_eq!(format_num(1_500), "1.5k");
        assert_eq!(format_num(2_500_000), "2.5M");
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("hi", 8), "hi");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }
}
