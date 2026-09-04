use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::browser::FileBrowser;

pub fn render_file_browser(frame: &mut Frame, area: Rect, browser: &FileBrowser) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let border_color = if browser.focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let title = format!(
        " Files {} ",
        browser
            .root
            .file_name()
            .unwrap_or(browser.root.as_os_str())
            .to_string_lossy()
    );
    let focus_hint = if browser.focused {
        " Enter: open  Tab/Esc: viewer "
    } else {
        " Tab: browse  e: hide "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(title, Style::default().fg(Color::Yellow)))
        .title_bottom(Span::styled(
            focus_hint,
            Style::default().fg(Color::DarkGray),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let message_rows = u16::from(browser.error.is_some() && inner.height > 2);
    let list_area = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(message_rows),
    );
    let visible = usize::from(list_area.height.max(1));
    let start = browser
        .selected
        .saturating_sub(visible / 2)
        .min(browser.entries.len().saturating_sub(visible));
    let end = (start + visible).min(browser.entries.len());
    let items = browser.entries[start..end]
        .iter()
        .map(|entry| {
            let current = entry.path == browser.current;
            ListItem::new(Line::from(vec![
                Span::styled(
                    if current { "● " } else { "  " },
                    Style::default().fg(Color::Green),
                ),
                Span::raw(&entry.label),
            ]))
        })
        .collect::<Vec<_>>();
    let list = List::new(items).highlight_symbol("› ").highlight_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default().with_selected(Some(browser.selected - start));
    frame.render_stateful_widget(list, list_area, &mut state);

    if let Some(error) = &browser.error {
        let error_area = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
        frame.render_widget(
            Paragraph::new(error.as_str()).style(Style::default().fg(Color::Red)),
            error_area,
        );
    }
}
