use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;

/// Render the keybinding hints bar at the bottom.
///
/// Ordered camera first (rotate, zoom, pan), then what is drawn (color, palette,
/// mode), then toggles, with `?` and `q` last: the two you look for when you are
/// done reading the rest.  Grouped keys -- `hjkl`, `wasd` -- rather than one
/// entry per direction, because the bar has to fit an 80-column terminal.
///
/// The hints change with the sequence panel: while it is open the arrow keys
/// drive the cursor rather than the camera, and saying so here is cheaper than
/// making the user open the help overlay to find out.
pub fn render_helpbar(frame: &mut Frame, area: Rect, app: &App) {
    if app.show_sequence {
        frame.render_widget(Paragraph::new(sequence_hints()), area);
        return;
    }
    let mut spans = vec![
        Span::styled("╰── ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "hjkl",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": rotate  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "+/-",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": zoom  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "wasd",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": pan  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "c",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": color  ", Style::default().fg(Color::Gray)),
    ];

    // Only advertise the palette key when the config defines palettes to cycle:
    // the bar is already full, and on a plain config `p` does nothing.  It sits
    // next to the color key, which is the one it is a variation on.
    if crate::config::palette_count() > 1 {
        spans.push(Span::styled(
            "p",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            ": palette  ",
            Style::default().fg(Color::Gray),
        ));
    }

    spans.extend([
        Span::styled(
            "v",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": mode  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "f",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": interface  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "I",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": interactions  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "g",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": ligands  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "S",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": sequence  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "?",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": help  ", Style::default().fg(Color::Gray)),
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": quit ", Style::default().fg(Color::Gray)),
        Span::styled("──╯", Style::default().fg(Color::DarkGray)),
    ]);

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Hints shown while the sequence panel has the arrow keys.
fn sequence_hints() -> Line<'static> {
    let key = |text: &'static str| {
        Span::styled(
            text,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    };
    let label = |text: &'static str| Span::styled(text, Style::default().fg(Color::Gray));

    Line::from(vec![
        Span::styled("╰── ", Style::default().fg(Color::DarkGray)),
        key("←→↑↓"),
        label(": cursor  "),
        key("shift+←→"),
        label(": range  "),
        key("↵"),
        label(": pick  "),
        key("A"),
        label(": chain  "),
        key("x"),
        label(": clear  "),
        key("b"),
        label(": ball&stick  "),
        key("z"),
        label(": centre  "),
        key("[ ]"),
        label(": chain  "),
        key("hjkl"),
        label(": rotate  "),
        key("<>"),
        label(": size  "),
        key("S/esc"),
        label(": close "),
        Span::styled("──╯", Style::default().fg(Color::DarkGray)),
    ])
}
