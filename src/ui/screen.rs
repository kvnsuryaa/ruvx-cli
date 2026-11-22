use super::*;
use ratatui::{
    Frame,
    layout::{Position as Cell, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use transcript::{Position, Row};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Button {
    Command(String),
    Choice(String),
    CommandChoice(String),
    Detail(usize),
    Bottom,
    Fold(usize),
    Paste(usize),
    SavePaste,
    RemovePaste,
    ClosePaste,
}
#[derive(Default)]
pub(super) struct FrameMap {
    pub transcript: Rect,
    pub rows: Vec<Row>,
    pub composer: Rect,
    pub command_menu: Rect,
    pub paste_body: Rect,
    pub editor_first: usize,
    pub buttons: Vec<(Rect, Button)>,
}
#[derive(Clone)]
pub(super) enum Gesture {
    Text {
        start: Position,
        start_end: Position,
        moved: bool,
    },
    Button {
        button: Button,
        column: u16,
        row: u16,
        moved: bool,
    },
}
fn label(frame: &mut Frame, area: Rect, value: impl Into<String>, style: Style) {
    if area.width > 0 && area.height > 0 {
        frame.render_widget(Paragraph::new(sanitize(&value.into())).style(style), area);
    }
}
pub(super) fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.frame = FrameMap::default();
    if area.width < 4 || area.height < 5 {
        label(frame, area, "Enlarge terminal", Style::default());
        return;
    }
    if app.paste_editor.is_some() {
        render_paste_editor(frame, app, area);
        return;
    }
    let accent = if app.mode == Mode::Plan {
        Color::Cyan
    } else {
        Color::Green
    };
    let title = format!("ruvx · {}", app.workspace);
    label(
        frame,
        Rect::new(1, 0, area.width.saturating_sub(2), 1),
        title,
        Style::default().add_modifier(Modifier::BOLD),
    );
    let has_models = app.config.providers.values().any(|p| {
        p.models
            .values()
            .any(|m| m.supports_tools != Some(false) && m.supports_streaming)
    });
    let model = app
        .selection
        .as_ref()
        .map(|s| format!("{}/{}", s.profile, s.model))
        .unwrap_or_else(|| {
            if has_models {
                "Choose model".into()
            } else {
                "/connect".into()
            }
        });
    let controls = vec![
        (
            format!("[{}]", app.mode),
            Button::Command(format!(
                "/mode {}",
                if app.mode == Mode::Plan {
                    "build"
                } else {
                    "plan"
                }
            )),
        ),
        (
            format!("[{model}]"),
            Button::Command(if app.selection.is_some() || has_models {
                "/model".into()
            } else {
                "/connect".into()
            }),
        ),
    ];
    let total: usize = controls.iter().map(|(s, _)| s.width() + 1).sum();
    if total + 15 < area.width as usize {
        let mut x = area.width - total as u16;
        for (text, button) in controls {
            let r = Rect::new(x, 0, text.width() as u16, 1);
            label(frame, r, text, Style::default().fg(accent));
            app.frame.buttons.push((r, button));
            x += r.width + 1;
        }
    } else {
        label(
            frame,
            Rect::new(1, 0, area.width - 2, 1),
            format!("ruvx · {} · {model}", app.mode),
            Style::default().fg(accent),
        );
    }
    let branch = app
        .git_branch
        .as_ref()
        .map(|branch| sanitize(&format!("git: {branch}")));
    let branch_width = branch.as_ref().map_or(0, |branch| {
        (branch.width() as u16).min(area.width.saturating_sub(2) / 3)
    });
    let status_width = area.width.saturating_sub(
        2 + if branch_width > 0 {
            branch_width + 2
        } else {
            0
        },
    );
    let activity = app.activity_status();
    let status_note = if activity.is_some() { "" } else { &app.status };
    let status = if app.selection.is_some() {
        let prefix = format!("[Effort: {}]", app.effort_label());
        let width = (prefix.width() as u16).min(status_width);
        if width > 0 && app.active.is_none() {
            app.frame
                .buttons
                .push((Rect::new(1, 1, width, 1), Button::Command("/effort".into())));
        }
        if status_note.is_empty() {
            prefix
        } else {
            format!("{prefix} · {status_note}")
        }
    } else {
        status_note.to_owned()
    };
    label(
        frame,
        Rect::new(1, 1, status_width, 1),
        status,
        Style::default(),
    );
    if let Some(branch) = branch
        && branch_width > 0
    {
        let mut text = String::new();
        if branch.width() <= branch_width as usize {
            text = branch;
        } else {
            for g in branch.graphemes(true) {
                if text.width() + g.width() + 1 > branch_width as usize {
                    break;
                }
                text.push_str(g);
            }
            text.push('…');
        }
        label(
            frame,
            Rect::new(area.width - branch_width - 1, 1, branch_width, 1),
            text,
            Style::default().fg(accent),
        );
    }
    let width = area.width.saturating_sub(4).max(1) as usize;
    let (editor_lines, cursor_row, cursor_column) = app.editor.display(width, app.dialog.masked());
    let input_height =
        (editor_lines.len().clamp(1, 6) as u16).min(area.height.saturating_sub(5).max(1));
    let menu = app.command_matches();
    let mut choices = app.dialog.choices();
    if let Dialog::ModelPick { choices: models } = &app.dialog {
        for ((label, _), selection) in choices.iter_mut().zip(models) {
            let effort = app
                .config
                .providers
                .get(&selection.profile)
                .and_then(|p| p.models.get(&selection.model))
                .and_then(|m| m.reasoning_effort)
                .map(|e| e.to_string())
                .unwrap_or_else(|| "default".into());
            label.push_str(&format!(" · effort {effort}"));
        }
    }
    app.focus = app.focus.min(choices.len().saturating_sub(1));
    let max_choices = if matches!(app.dialog, Dialog::EffortPick { .. }) {
        9
    } else {
        6
    };
    let choice_count = menu
        .as_ref()
        .map_or(choices.len(), |items| items.len().max(1));
    let choice_height =
        (choice_count.min(max_choices) as u16).min(area.height.saturating_sub(input_height + 6));
    let composer_y = area.height.saturating_sub(input_height + 1);
    app.frame.composer = Rect::new(2, composer_y, area.width - 4, input_height);
    let dialog_y = composer_y.saturating_sub(choice_height + 1);
    let activity_y = activity.as_ref().map(|_| dialog_y.saturating_sub(1).max(2));
    let body_y = 3.min(dialog_y);
    let body_bottom = activity_y.unwrap_or(dialog_y).saturating_sub(1).max(body_y);
    app.frame.transcript = Rect::new(2, body_y, area.width - 4, body_bottom - body_y);
    let heading = app
        .overlay
        .as_ref()
        .map(|o| format!("{} · Esc close · PgUp/PgDn scroll", o.title))
        .unwrap_or_else(|| "Conversation".into());
    label(
        frame,
        Rect::new(1, 2, area.width - 2, 1),
        heading,
        Style::default().fg(accent),
    );
    let rect = app.frame.transcript;
    let rows = app
        .view()
        .visible(rect.width as usize, rect.height as usize);
    if app.overlay.is_none()
        && let Some(row) = rows.first()
        && !row.label
        && matches!(
            row.role,
            transcript::Role::User | transcript::Role::Assistant
        )
    {
        label(
            frame,
            Rect::new(1, 2, area.width - 2, 1),
            format!("Conversation · {} (continued)", row.role.label()),
            Style::default().fg(accent),
        );
    }
    let selected = app.view().selection.map(|s| s.range());
    for (index, row) in rows.iter().enumerate() {
        let mut r = Rect::new(rect.x, rect.y + index as u16, rect.width, 1);
        let hit = r;
        let background = if row.role == transcript::Role::User {
            Style::default()
                .bg(Color::Indexed(236))
                .fg(Color::Indexed(252))
        } else {
            Style::default()
        };
        frame.buffer_mut().set_style(hit, background);
        if row.inset > 0 {
            r.x += row.inset as u16;
            r.width = r.width.saturating_sub((row.inset * 2) as u16);
        }
        let mut spans = vec![];
        if let Some(range) = &row.range {
            for (offset, g) in row.text.grapheme_indices(true) {
                let pos = Position {
                    block: row.block,
                    byte: range.start + offset,
                };
                let highlighted = selected.is_some_and(|(start, end)| pos >= start && pos < end);
                let style = if highlighted {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else if row.role == transcript::Role::System && app.overlay.is_none() {
                    Style::default().add_modifier(Modifier::DIM)
                } else {
                    Style::default()
                };
                spans.push(Span::styled(
                    if g == "\t" {
                        "    ".to_owned()
                    } else {
                        g.to_owned()
                    },
                    style,
                ));
            }
        } else {
            let text = row.text.clone();
            let style = match row.role {
                transcript::Role::User => background.add_modifier(if row.label {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
                transcript::Role::Assistant if row.label => {
                    Style::default().add_modifier(Modifier::BOLD)
                }
                transcript::Role::Tool => Style::default().add_modifier(Modifier::DIM),
                transcript::Role::System => Style::default().add_modifier(Modifier::DIM),
                _ => Style::default(),
            };
            spans.push(Span::styled(text, style));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)).style(background), r);
        if row.detail {
            app.frame.buttons.push((hit, Button::Detail(row.block)));
        }
        if row.fold {
            app.frame.buttons.push((hit, Button::Fold(row.block)));
        }
    }
    app.frame.rows = rows;
    if app.overlay.is_none() && !app.transcript.follow && activity_y != Some(body_bottom) {
        let text = format!("[Latest · {} new]", app.transcript.new_blocks);
        let r = Rect::new(2, body_bottom, area.width - 4, 1);
        label(frame, r, text, Style::default().fg(accent));
        app.frame.buttons.push((r, Button::Bottom));
    }
    let hint = if menu.is_some() {
        if app.active.is_some() {
            "Commands unavailable · wait or Ctrl+C to cancel · Tab completes · Esc closes"
        } else {
            "Commands · Up/Down selects · Enter opens · Tab completes · Esc closes"
        }
        .to_owned()
    } else if matches!(app.dialog, Dialog::None)
        && app.active.is_some()
        && app.activity.draft_waiting
    {
        "AI is still working; draft kept. Send again when ready.".to_owned()
    } else if matches!(app.dialog, Dialog::None) {
        let name = app.selection.as_ref().map_or(model.as_str(), |selection| {
            app.config
                .providers
                .get(&selection.profile)
                .and_then(|profile| profile.models.get(&selection.model))
                .map(|model| model.name.trim())
                .filter(|name| !name.is_empty())
                .unwrap_or(&selection.model)
        });
        if app.editor.pastes.is_empty() {
            format!("{name} · {} · Shift+Tab mode · /help", app.mode)
        } else {
            format!(
                "{name} · {} · F2/click edits paste · Shift+Tab mode · /help",
                app.mode
            )
        }
    } else if matches!(app.dialog, Dialog::Approval { .. }) {
        app.activity
            .approval_hint
            .unwrap_or(app.dialog.label())
            .to_owned()
    } else {
        app.dialog.label().to_owned()
    };
    label(
        frame,
        Rect::new(1, dialog_y, area.width - 2, 1),
        hint,
        Style::default().fg(accent),
    );
    if let (Some(activity), Some(y)) = (activity, activity_y) {
        label(
            frame,
            Rect::new(1, y, area.width - 2, 1),
            activity,
            Style::default().fg(accent),
        );
    }
    if menu.is_none()
        && app.context_blocked
        && app.active.is_none()
        && matches!(app.dialog, Dialog::None)
    {
        label(
            frame,
            Rect::new(1, dialog_y, area.width - 2, 1),
            "[Compact context]  [New session]  F2 edits paste",
            Style::default().fg(Color::Cyan),
        );
        app.frame.buttons.push((
            Rect::new(1, dialog_y, 17.min(area.width - 2), 1),
            Button::Command("/compact".into()),
        ));
        if area.width > 21 {
            app.frame.buttons.push((
                Rect::new(20, dialog_y, 13.min(area.width - 20), 1),
                Button::Command("/new".into()),
            ));
        }
    }
    let first_choice = app
        .focus
        .saturating_sub(choice_height.saturating_sub(1) as usize);
    for (offset, (text, value)) in choices
        .iter()
        .enumerate()
        .skip(first_choice)
        .take(choice_height as usize)
    {
        let r = Rect::new(
            2,
            dialog_y + 1 + (offset - first_choice) as u16,
            area.width - 4,
            1,
        );
        let focused = offset == app.focus;
        let prefix = if focused { "> " } else { "  " };
        label(
            frame,
            r,
            format!("{prefix}{text}"),
            if focused {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            },
        );
        app.frame.buttons.push((r, Button::Choice(value.clone())));
    }
    if let Some(items) = &menu {
        let panel = Rect::new(2, dialog_y + 1, area.width - 4, choice_height);
        app.frame.command_menu = Rect::new(1, dialog_y, area.width - 2, choice_height + 1);
        let surface = Style::default()
            .bg(Color::Indexed(236))
            .fg(Color::Indexed(252));
        frame.buffer_mut().set_style(panel, surface);
        let first = app
            .command_menu
            .selected
            .saturating_sub(choice_height.saturating_sub(1) as usize);
        if items.is_empty() {
            label(
                frame,
                Rect::new(panel.x, panel.y, panel.width, choice_height.min(1)),
                "  No matching commands",
                surface,
            );
        }
        for (index, command) in items
            .iter()
            .enumerate()
            .skip(first)
            .take(choice_height as usize)
        {
            let r = Rect::new(panel.x, panel.y + (index - first) as u16, panel.width, 1);
            let selected = index == app.command_menu.selected;
            let style = if app.active.is_some() {
                surface.add_modifier(Modifier::DIM)
            } else if selected {
                surface.add_modifier(Modifier::REVERSED)
            } else {
                surface
            };
            let prefix = if selected { ">" } else { " " };
            label(
                frame,
                r,
                format!("{prefix} {:<13} {}", command.name, command.description),
                style,
            );
            if app.active.is_none() {
                app.frame
                    .buttons
                    .push((r, Button::CommandChoice(command.name.into())));
            }
        }
        if items.len() > choice_height as usize && choice_height > 0 {
            let count = format!(" {}/{} ", app.command_menu.selected + 1, items.len());
            let w = (count.width() as u16).min(area.width - 2);
            label(
                frame,
                Rect::new(area.width - w - 1, dialog_y, w, 1),
                count,
                Style::default().fg(accent),
            );
        }
    }
    let first = cursor_row.saturating_sub(input_height as usize - 1);
    app.frame.editor_first = first;
    for (index, line) in editor_lines
        .iter()
        .skip(first)
        .take(input_height as usize)
        .enumerate()
    {
        label(
            frame,
            Rect::new(2, composer_y + index as u16, area.width - 4, 1),
            line.clone(),
            Style::default(),
        );
    }
    if matches!(app.dialog, Dialog::None) {
        for (id, row, col, len) in app.editor.paste_cells(width) {
            if row >= first && row < first + input_height as usize {
                let r = Rect::new(
                    2 + col as u16,
                    composer_y + (row - first) as u16,
                    (len as u16).min(area.width.saturating_sub(2 + col as u16)),
                    1,
                );
                frame.buffer_mut().set_style(
                    r,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::UNDERLINED),
                );
                app.frame.buttons.push((r, Button::Paste(id)));
            }
        }
    }
    label(
        frame,
        Rect::new(0, composer_y, 2, 1),
        "> ",
        Style::default().fg(accent),
    );
    frame.set_cursor_position((
        2 + (cursor_column as u16).min(area.width - 4),
        composer_y + ((cursor_row - first) as u16).min(input_height - 1),
    ));
    let footer = if app.view().selection.is_some() {
        "Selection · release to copy · Ctrl+C copy/clear · Esc clear"
    } else {
        "PgUp/PgDn · wheel scroll · drag to copy · Ctrl+C cancel"
    };
    frame.render_widget(
        Paragraph::new(footer).wrap(Wrap { trim: false }),
        Rect::new(1, area.height - 1, area.width - 2, 1),
    );
    if let Some(button) = &app.focused_button {
        if let Some((rect, _)) = app.frame.buttons.iter().find(|(_, b)| b == button) {
            frame
                .buffer_mut()
                .set_style(*rect, Style::default().add_modifier(Modifier::REVERSED));
        } else {
            app.focused_button = None;
        }
    }
}

fn render_paste_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    let activity = app.activity_status();
    let editor = app.paste_editor.as_mut().unwrap();
    label(
        frame,
        Rect::new(1, 0, area.width - 2, 1),
        format!("Edit Paste {}", editor.id),
        Style::default().add_modifier(Modifier::BOLD),
    );
    let buttons = [
        ("[Save · Ctrl+S]", Button::SavePaste),
        ("[Cancel · Esc]", Button::ClosePaste),
        ("[Remove · Ctrl+D]", Button::RemovePaste),
    ];
    let mut x = 1;
    for (text, button) in buttons {
        let width = (text.width() as u16).min(area.width.saturating_sub(x));
        if width > 0 {
            let r = Rect::new(x, 1, width, 1);
            label(frame, r, text, Style::default().fg(Color::Cyan));
            app.frame.buttons.push((r, button));
            x += width + 1;
        }
    }
    let rect = Rect::new(
        2,
        3,
        area.width - 4,
        area.height
            .saturating_sub(if activity.is_some() { 5 } else { 4 }),
    );
    if let Some(activity) = activity {
        label(
            frame,
            Rect::new(1, area.height - 2, area.width - 2, 1),
            activity,
            Style::default().fg(Color::Cyan),
        );
    }
    app.frame.paste_body = rect;
    match editor.display(rect.width as usize, rect.height as usize) {
        Ok((lines, cursor)) => {
            for (row, text) in lines.into_iter().take(rect.height as usize).enumerate() {
                label(
                    frame,
                    Rect::new(rect.x, rect.y + row as u16, rect.width, 1),
                    text,
                    Style::default(),
                );
            }
            if let Some((x, y)) = cursor
                && rect.height > 0
            {
                frame.set_cursor_position((
                    rect.x + (x as u16).min(rect.width.saturating_sub(1)),
                    rect.y + y as u16,
                ));
            }
        }
        Err(error) => label(
            frame,
            rect,
            format!("Unable to read paste: {error}"),
            Style::default(),
        ),
    }
    label(
        frame,
        Rect::new(1, area.height - 1, area.width - 2, 1),
        "Edit full text · PgUp/PgDn/wheel scroll · changes apply on Save",
        Style::default().add_modifier(Modifier::DIM),
    );
}

impl Dialog {
    pub(super) fn choices(&self) -> Vec<(String, String)> {
        let values: Vec<(String, String)> = match self {
            Self::Extension(setup) => setup.choices(),
            Self::Provider => vec![
                ("1 OpenAI API".into(), "1".into()),
                ("2 Continue with ChatGPT".into(), "2".into()),
                ("3 Custom compatible".into(), "3".into()),
            ],
            Self::AuthMethod { draft } => {
                let mut items = vec![
                    ("1 Saved API key".into(), "1".into()),
                    ("2 Environment variable".into(), "2".into()),
                ];
                if draft.kind == ProviderKind::Compatible {
                    items.push(("3 None (local endpoint)".into(), "3".into()));
                }
                items
            }
            Self::ModelPick { choices } => choices
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (
                        format!("{} {}/{}", i + 1, s.profile, s.model),
                        (i + 1).to_string(),
                    )
                })
                .collect(),
            Self::EffortPick { levels, .. } => {
                let mut choices = vec![("Provider default".into(), "default".into())];
                choices.extend(levels.iter().map(|level| {
                    (
                        format!("{level} · {}", level.description()),
                        level.to_string(),
                    )
                }));
                choices.push(("Configure supported levels…".into(), "configure".into()));
                choices
            }
            Self::DisconnectPick { ids } => ids
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("{} {s}", i + 1), (i + 1).to_string()))
                .collect(),
            Self::ModelTools { .. } => vec![
                ("Yes, tools and streaming".into(), "yes".into()),
                ("No".into(), "no".into()),
            ],
            Self::Approval { .. } => vec![
                ("Deny".into(), "deny".into()),
                ("Run once".into(), "run".into()),
            ],
            _ => vec![],
        };
        values
    }
}
impl App {
    pub(super) fn activate(&mut self, button: Button, height: u16) -> Result<()> {
        match button {
            Button::Bottom => self.view().bottom(),
            Button::Fold(id) => self.view().toggle(id)?,
            Button::Paste(id) => self.open_paste(id)?,
            Button::SavePaste => self.save_paste()?,
            Button::ClosePaste => self.paste_editor = None,
            Button::RemovePaste => {
                if let Some(editor) = self.paste_editor.take() {
                    self.editor.remove_paste(editor.id);
                    self.invalidate_preflight();
                }
            }
            Button::Command(text) => {
                ensure!(
                    matches!(self.dialog, Dialog::None),
                    "Finish or close the current dialog first"
                );
                let preserve = matches!(
                    text.as_str(),
                    "/model" | "/connect" | "/disconnect" | "/effort"
                );
                if preserve && self.saved_draft.is_none() && !self.editor.text.is_empty() {
                    self.saved_draft = Some(self.editor.take_snapshot());
                }
                if let Err(error) = self.command(&text, height) {
                    if let Some(draft) = self.saved_draft.take() {
                        self.editor.restore(draft);
                    }
                    return Err(error);
                }
                self.focus = 0;
            }
            Button::CommandChoice(name) => self.choose_command(&name, false, height)?,
            Button::Choice(value) => {
                // Input can contain several clicks before the next frame. A choice
                // from the previous dialog must not overwrite a restored draft.
                if self
                    .dialog
                    .choices()
                    .iter()
                    .any(|(_, current)| current == &value)
                {
                    self.editor.set(value);
                    self.submit(height)?;
                }
            }
            Button::Detail(id) => {
                let text = self.view().detail_text(id)?;
                self.show_text_overlay("Detail", text)?;
            }
        }
        Ok(())
    }
    pub(super) fn scroll_view(&mut self, delta: isize) {
        let r = self.frame.transcript;
        self.view()
            .scroll(delta, r.width.max(1) as usize, r.height as usize);
    }
    fn text_position(&self, column: u16, row: u16, inclusive: bool) -> Option<Position> {
        let r = self.frame.transcript;
        if r.height == 0 {
            return None;
        }
        let index = row.clamp(r.y, r.bottom() - 1).saturating_sub(r.y) as usize;
        let index = index.min(self.frame.rows.len().saturating_sub(1));
        let row = self
            .frame
            .rows
            .get(index)
            .filter(|row| row.range.is_some())
            .or_else(|| {
                self.frame.rows[..index]
                    .iter()
                    .rev()
                    .find(|row| row.range.is_some())
            })
            .or_else(|| {
                self.frame
                    .rows
                    .iter()
                    .skip(index)
                    .find(|row| row.range.is_some())
            })?;
        row.position(column.saturating_sub(r.x) as usize, inclusive)
    }
    fn select_to(&mut self, column: u16, row: u16) {
        let Some(Gesture::Text {
            start, start_end, ..
        }) = self.gesture.clone()
        else {
            return;
        };
        let Some(end) = self.text_position(column, row, false) else {
            return;
        };
        let (anchor, end) = if end < start {
            (start_end, end)
        } else {
            (start, self.text_position(column, row, true).unwrap_or(end))
        };
        self.view().selection = Some(TextSelection { anchor, end });
    }
    pub(super) fn drag_scroll(&mut self) -> bool {
        if !matches!(self.gesture, Some(Gesture::Text { moved: true, .. }))
            || self.last_drag_scroll.elapsed() < Duration::from_millis(80)
        {
            return false;
        }
        let r = self.frame.transcript;
        let (x, y) = self.pointer;
        let delta = if y <= r.y {
            -1
        } else if y >= r.bottom().saturating_sub(1) {
            1
        } else {
            return false;
        };
        self.scroll_view(delta);
        let rows = self
            .view()
            .visible(r.width.max(1) as usize, r.height as usize);
        self.frame.rows = rows;
        self.select_to(x, y);
        self.last_drag_scroll = std::time::Instant::now();
        true
    }
    pub(super) fn mouse(&mut self, event: MouseEvent, height: u16) -> Result<()> {
        let cell = Cell::new(event.column, event.row);
        self.pointer = (event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollUp => {
                if self.frame.command_menu.contains(cell) && self.command_matches().is_some() {
                    self.move_command(false);
                } else if let Some(editor) = &mut self.paste_editor {
                    editor.scroll(-3);
                } else {
                    self.scroll_view(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.frame.command_menu.contains(cell) && self.command_matches().is_some() {
                    self.move_command(true);
                } else if let Some(editor) = &mut self.paste_editor {
                    editor.scroll(3);
                } else {
                    self.scroll_view(3);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.view().selection = None;
                self.gesture = None;
                self.focused_button = None;
                if let Some((_, button)) = self
                    .frame
                    .buttons
                    .iter()
                    .rev()
                    .find(|(rect, _)| rect.contains(cell))
                {
                    self.gesture = Some(Gesture::Button {
                        button: button.clone(),
                        column: event.column,
                        row: event.row,
                        moved: false,
                    });
                } else if self.frame.paste_body.contains(cell) && self.paste_editor.is_some() {
                    let r = self.frame.paste_body;
                    self.paste_editor
                        .as_mut()
                        .unwrap()
                        .click((event.column - r.x) as usize, (event.row - r.y) as usize)?;
                } else if self.frame.transcript.contains(cell) {
                    if let (Some(start), Some(start_end)) = (
                        self.text_position(event.column, event.row, false),
                        self.text_position(event.column, event.row, true),
                    ) {
                        self.gesture = Some(Gesture::Text {
                            start,
                            start_end,
                            moved: false,
                        });
                    }
                } else if self.frame.composer.contains(cell) {
                    let r = self.frame.composer;
                    self.editor.set_cell(
                        r.width as usize,
                        self.frame.editor_first + (event.row - r.y) as usize,
                        (event.column - r.x) as usize,
                        self.dialog.masked(),
                    );
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match &mut self.gesture {
                Some(Gesture::Button {
                    column, row, moved, ..
                }) => {
                    *moved |= *column != event.column || *row != event.row;
                }
                Some(Gesture::Text { moved, .. }) => {
                    *moved = true;
                    self.select_to(event.column, event.row);
                }
                _ => {}
            },
            MouseEventKind::Up(MouseButton::Left) => {
                if matches!(self.gesture, Some(Gesture::Text { moved: true, .. })) {
                    self.select_to(event.column, event.row);
                }
                match self.gesture.take() {
                    Some(Gesture::Text { moved: true, .. }) => {
                        self.copy_selection(false)?;
                    }
                    Some(Gesture::Button {
                        button,
                        moved: false,
                        ..
                    }) => {
                        if self
                            .frame
                            .buttons
                            .iter()
                            .any(|(r, b)| r.contains(cell) && *b == button)
                        {
                            self.activate(button, height)?;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }
}
