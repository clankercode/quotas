use super::detail::DetailView;
use super::provider_card::ProviderCard;
use crate::providers::ProviderResult;
use ratatui::layout::Rect;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

#[derive(Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

pub struct Dashboard {
    pub results: Vec<ProviderResult>,
    pub selected_index: usize,
    pub cols: usize,
    pub show_detail: bool,
}

impl Dashboard {
    pub fn new(results: Vec<ProviderResult>) -> Self {
        Self {
            results,
            selected_index: 0,
            cols: 3,
            show_detail: false,
        }
    }

    pub fn visible_providers(&self) -> Vec<&ProviderResult> {
        self.results.iter().collect()
    }

    pub fn selected_provider(&self) -> Option<&ProviderResult> {
        let visible = self.visible_providers();
        visible.get(self.selected_index).copied()
    }

    pub fn navigate(&mut self, dir: Direction) {
        let visible = self.visible_providers();
        if visible.is_empty() {
            return;
        }
        let cols = self.cols.min(visible.len());
        let _rows = visible.len().div_ceil(cols);
        let col = self.selected_index % cols;
        let row = self.selected_index / cols;

        match dir {
            Direction::Left => {
                if col > 0 {
                    self.selected_index -= 1;
                }
            }
            Direction::Right => {
                if col < cols - 1 && self.selected_index + 1 < visible.len() {
                    self.selected_index += 1;
                }
            }
            Direction::Up => {
                if row > 0 {
                    self.selected_index = self.selected_index.saturating_sub(cols);
                }
            }
            Direction::Down => {
                let next_row = row + 1;
                let max_idx = visible.len() - 1;
                if next_row * cols <= max_idx {
                    self.selected_index += cols;
                } else if self.selected_index < max_idx {
                    self.selected_index = max_idx;
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

    fn render_grid(&self, f: &mut Frame) {
        let area = f.size();
        let visible = self.visible_providers();

        if visible.is_empty() {
            let paragraph = Paragraph::new(
                "No providers configured. Set API keys via environment variables or config files.",
            )
            .block(Block::new().borders(Borders::ALL).title("quotas"));
            f.render_widget(paragraph, area);
            return;
        }

        let cols = self.cols.min(visible.len());
        let rows = visible.len().div_ceil(cols);

        let title_area = Rect::new(area.x, area.y, area.width, 3);
        let grid_area = Rect::new(
            area.x,
            area.y + 3,
            area.width,
            area.height.saturating_sub(4),
        );
        let footer_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            1,
        );

        let title = Paragraph::new(vec![
            Line::from(vec![Span::raw(" quotas ").bold().white()]),
            Line::from(vec![Span::raw(
                " ←↑↓→ Navigate  Enter Detail  R Refresh  C Copy  Q Quit ",
            )
            .dim()]),
        ])
        .block(Block::new().borders(Borders::NONE));
        f.render_widget(title, title_area);

        let footer = Paragraph::new("Press ? for help").style(Style::new().dim());
        f.render_widget(footer, footer_area);

        let card_width = grid_area.width / cols as u16;
        let card_height = (grid_area.height / rows as u16).max(8);

        for (idx, provider) in visible.iter().enumerate() {
            let col_idx = idx % cols;
            let row_idx = idx / cols;

            let x = grid_area.x + col_idx as u16 * card_width;
            let y = grid_area.y + row_idx as u16 * card_height;

            let card_area = Rect::new(x, y, card_width.saturating_sub(1), card_height);
            self.render_card(f, provider, idx == self.selected_index, card_area);
        }
    }

    fn render_card(&self, f: &mut Frame, result: &ProviderResult, selected: bool, area: Rect) {
        let card = ProviderCard::new(result.clone(), selected);
        let freshness = card.freshness_label();

        let border_style = if selected {
            Style::new().green()
        } else {
            Style::new().dim()
        };

        let freshness_style = match freshness.staleness {
            crate::tui::freshness::Staleness::Fresh => Style::new().cyan(),
            crate::tui::freshness::Staleness::Warning => Style::new().yellow(),
            crate::tui::freshness::Staleness::Stale => Style::new().red(),
        };

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![Span::raw(format!(
            "{} {}",
            if selected { "▶" } else { " " },
            card.display_name()
        ))
        .bold()]));
        lines.push(Line::from(vec![Span::styled(
            &freshness.label,
            freshness_style,
        )]));

        let primary = card.primary_label();
        let primary_colored = if card.available() {
            Span::raw(&primary)
        } else {
            Span::raw(&primary).red()
        };
        lines.push(Line::from(vec![primary_colored]));

        for sec in card.secondary_lines() {
            let sec_span = Span::from(sec).dim();
            lines.push(Line::from(vec![sec_span]));
        }

        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(border_style);

        let paragraph = Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true })
            .alignment(ratatui::layout::Alignment::Left);

        f.render_widget(paragraph, area);
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
            let text = view.render();

            let detail_area = Rect::new(
                area.x,
                area.y + 2,
                area.width,
                area.height.saturating_sub(3),
            );
            let paragraph = Paragraph::new(text)
                .block(Block::new().borders(Borders::NONE))
                .wrap(Wrap { trim: true })
                .scroll((0, 0));
            f.render_widget(paragraph, detail_area);
        }
    }
}
