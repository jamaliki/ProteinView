use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::browser::FileBrowser;

#[derive(Clone, Copy)]
enum InputMode {
    Editor,
    ProteinView,
}

/// Render one compact, focus-aware mode line.
///
/// The complete binding list belongs in `?` help. This line adds hints only
/// while they fit, so opening the file browser never produces clipped text.
pub fn render_statusbar(frame: &mut Frame, area: Rect, browser: Option<&FileBrowser>) {
    let mode = if browser.is_some_and(|browser| browser.focused) {
        InputMode::Editor
    } else {
        InputMode::ProteinView
    };
    let browser_available = browser.is_some();
    let mut spans = vec![mode_span(mode)];
    for (key, action) in fitting_hints(area.width, mode, browser_available) {
        spans.push(Span::styled(
            format!(" {key}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        if !action.is_empty() {
            spans.push(Span::styled(
                format!(" {action}"),
                Style::default().fg(Color::Gray),
            ));
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Rgb(24, 24, 28))),
        area,
    );
}

fn mode_span(mode: InputMode) -> Span<'static> {
    let (label, color) = match mode {
        InputMode::Editor => (" EDITOR ", Color::Yellow),
        InputMode::ProteinView => (" PROTEINVIEW ", Color::Green),
    };
    Span::styled(
        label,
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn fitting_hints(
    width: u16,
    mode: InputMode,
    browser_available: bool,
) -> Vec<(&'static str, &'static str)> {
    let mode_width = match mode {
        InputMode::Editor => 8,
        InputMode::ProteinView => 13,
    };
    let mut used = mode_width;
    let mut result = Vec::new();
    let candidates: &[(&str, &str)] = match mode {
        InputMode::Editor => &[
            ("j/k", "move"),
            ("Enter", "open"),
            ("Tab", "viewer"),
            ("e", "hide"),
            ("PgUp/Dn", "page"),
        ],
        InputMode::ProteinView if browser_available => &[
            ("hjkl", "rotate"),
            ("wasd", "pan"),
            ("+/-", "zoom"),
            ("Tab", "editor"),
            ("e", "files"),
        ],
        InputMode::ProteinView => &[
            ("hjkl", "rotate"),
            ("wasd", "pan"),
            ("+/-", "zoom"),
            ("m/M", "quality"),
        ],
    };
    let verbose_essential = [("?", "help"), ("q", "quit")];
    let compact_essential = [("?", ""), ("q", "")];
    let essential = if mode_width + hints_width(&verbose_essential) <= usize::from(width) {
        &verbose_essential
    } else {
        &compact_essential
    };
    let essential_width = hints_width(essential);

    for &(key, action) in candidates {
        let hint_width = hint_width(key, action);
        if used + hint_width + essential_width > usize::from(width) {
            break;
        }
        result.push((key, action));
        used += hint_width;
    }
    result.extend(essential.iter().copied());
    result
}

fn hints_width(hints: &[(&str, &str)]) -> usize {
    hints
        .iter()
        .map(|(key, action)| hint_width(key, action))
        .sum()
}

fn hint_width(key: &str, action: &str) -> usize {
    1 + key.len() + usize::from(!action.is_empty()) * (1 + action.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_never_exceed_the_available_width() {
        for width in 17..=160 {
            for mode in [InputMode::Editor, InputMode::ProteinView] {
                let mode_width = match mode {
                    InputMode::Editor => 8,
                    InputMode::ProteinView => 13,
                };
                let hints = fitting_hints(width, mode, true);
                assert!(mode_width + hints_width(&hints) <= usize::from(width));
            }
        }
    }
}
