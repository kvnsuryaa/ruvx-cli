//! Passive, searchable MCP configuration browser. Never starts a server or agent.
use super::*;
use crate::config::McpServerConfig;
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use std::{collections::BTreeMap, path::PathBuf};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Servers,
    Configuration,
}
#[derive(Clone, Copy)]
enum Hit {
    Tab(Tab),
    Server(usize),
}

pub(super) struct Browser {
    servers: Vec<(String, McpServerConfig)>,
    config_path: PathBuf,
    query: Editor,
    matches: Vec<usize>,
    selected: usize,
    top: usize,
    tab: Tab,
    expanded: bool,
    scroll: usize,
    max_scroll: usize,
    page_size: usize,
    hits: Vec<(Rect, Hit)>,
}
impl Browser {
    pub(super) fn new(servers: &BTreeMap<String, McpServerConfig>, config_path: PathBuf) -> Self {
        Self {
            servers: servers
                .iter()
                .map(|(id, server)| (id.clone(), server.clone()))
                .collect(),
            config_path,
            query: Editor::default(),
            matches: (0..servers.len()).collect(),
            selected: 0,
            top: 0,
            tab: Tab::Servers,
            expanded: false,
            scroll: 0,
            max_scroll: 0,
            page_size: 1,
            hits: vec![],
        }
    }
    fn filter(&mut self) {
        let query = self.query.text.to_lowercase();
        self.matches = self
            .servers
            .iter()
            .enumerate()
            .filter_map(|(index, (id, server))| {
                (id.to_lowercase().contains(&query)
                    || server
                        .command
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&query))
                .then_some(index)
            })
            .collect();
        self.selected = 0;
        self.top = 0;
        self.scroll = 0;
    }
    pub(super) fn paste(&mut self, text: &str) {
        if self.tab != Tab::Servers || self.expanded {
            return;
        }
        let text = sanitize(text)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let remaining = 1024usize.saturating_sub(self.query.text.len());
        let text: String = text
            .chars()
            .scan(0, |bytes, ch| {
                *bytes += ch.len_utf8();
                (*bytes <= remaining).then_some(ch)
            })
            .collect();
        self.query.insert(&text);
        self.filter();
    }
    fn move_selection(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.matches.len().saturating_sub(1));
        self.scroll = 0;
    }
    fn change_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.expanded = false;
        self.scroll = 0;
        self.max_scroll = 0;
    }
    fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll);
    }
    /// Returns true when the user closes the browser.
    pub(super) fn key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.code == KeyCode::Esc {
            if self.expanded {
                self.expanded = false;
                self.scroll = 0;
                return false;
            }
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.change_tab(if self.tab == Tab::Servers {
                Tab::Configuration
            } else {
                Tab::Servers
            });
            return false;
        }
        if self.expanded || self.tab != Tab::Servers {
            match key.code {
                KeyCode::Up => self.scroll_by(-1),
                KeyCode::Down => self.scroll_by(1),
                KeyCode::PageUp => self.scroll_by(-(self.page_size as isize)),
                KeyCode::PageDown => self.scroll_by(self.page_size as isize),
                KeyCode::Home => self.scroll = 0,
                KeyCode::End => self.scroll = self.max_scroll,
                KeyCode::Enter if self.expanded => {
                    self.expanded = false;
                    self.scroll = 0;
                }
                _ => {}
            }
            return false;
        }
        let revision = self.query.revision;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Home => self.move_selection(-(self.selected as isize)),
                KeyCode::End => self.move_selection(self.matches.len() as isize),
                _ => {
                    if let Some(kind) = deletion::key(key) {
                        self.query.erase_backward(kind);
                    }
                }
            }
        } else if let Some(kind) = deletion::key(key) {
            self.query.erase_backward(kind);
        } else {
            match key.code {
                KeyCode::Up => self.move_selection(-1),
                KeyCode::Down => self.move_selection(1),
                KeyCode::PageUp => self.move_selection(-(self.page_size as isize)),
                KeyCode::PageDown => self.move_selection(self.page_size as isize),
                KeyCode::Enter if !self.matches.is_empty() => {
                    self.expanded = true;
                    self.scroll = 0;
                }
                KeyCode::Backspace => self.query.backspace(),
                KeyCode::Delete => self.query.delete(),
                KeyCode::Left => self.query.left(),
                KeyCode::Right => self.query.right(),
                KeyCode::Home => self.query.home(),
                KeyCode::End => self.query.end(),
                KeyCode::Char(ch)
                    if !key.modifiers.intersects(
                        KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META,
                    ) && !ch.is_control()
                        && self.query.text.len() + ch.len_utf8() <= 1024 =>
                {
                    self.query.insert(&ch.to_string());
                }
                _ => {}
            }
        }
        if self.query.revision != revision {
            self.filter();
        }
        false
    }
    pub(super) fn mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if event.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                if self.tab == Tab::Servers && !self.expanded {
                    self.move_selection(delta);
                } else {
                    self.scroll_by(delta);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let cell = Position::new(event.column, event.row);
                if let Some(hit) = self
                    .hits
                    .iter()
                    .find(|(rect, _)| rect.contains(cell))
                    .map(|(_, hit)| *hit)
                {
                    match hit {
                        Hit::Tab(tab) => self.change_tab(tab),
                        Hit::Server(index) if index < self.matches.len() => {
                            self.selected = index;
                            self.scroll = 0;
                        }
                        Hit::Server(_) => {}
                    }
                }
            }
            _ => {}
        }
    }
    fn detail(&self) -> String {
        let Some((id, server)) = self.matches.get(self.selected).map(|&i| &self.servers[i]) else {
            return String::new();
        };
        // Show environment names only; never inspect process environment values.
        format!(
            "{} [{}]\n\nTransport: stdio\nExecutable: {}\nArguments: {}\nEnvironment names: {}\nTimeout: {}s\nMessage limit: {} bytes\n\nStatus is configuration, not connection state.\nNo server is started by this browser.",
            sanitize(id),
            status(server),
            sanitize(&server.command.display().to_string()),
            serde_json::to_string(&server.args).expect("string array"),
            serde_json::to_string(&server.inherit_env).expect("string array"),
            server.timeout_seconds,
            server.max_message_bytes,
        )
    }
    fn configuration(&self) -> String {
        format!(
            "Global configuration\n{}\n\nTransport: stdio only.\n\nManage after closing this browser:\n/mcp add\n/mcp edit <id>\n/mcp remove <id>\n/mcp enable <id>\n/mcp disable <id>\n\nChanges apply to the next agent run. Enabled/disabled describes configuration, not connection state.\n\nBuild operations follow the active Ask/BYPASS approval policy. Servers stop when the agent run ends.\n\nThis browser only displays configuration. No model requests, tool discovery, or server processes are started.",
            sanitize(&self.config_path.display().to_string())
        )
    }
    pub(super) fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.hits.clear();
        let width = area.width.saturating_sub(2);
        let accent = Style::default().fg(Color::Cyan);
        let dim = Style::default().add_modifier(Modifier::DIM);
        line(
            frame,
            Rect::new(1, 0, width, 1),
            "MCP Servers",
            Style::default().add_modifier(Modifier::BOLD),
        );
        let enabled = self.servers.iter().filter(|(_, s)| s.enabled).count();
        let summary = format!(
            "{} servers · {} enabled · {} disabled · stdio only",
            self.servers.len(),
            enabled,
            self.servers.len() - enabled
        );
        line(frame, Rect::new(1, 1, width, 1), &summary, dim);
        let mut x = 1;
        for (tab, title) in [
            (Tab::Servers, "Servers"),
            (Tab::Configuration, "Configuration"),
        ] {
            if x >= area.width.saturating_sub(1) {
                break;
            }
            let text = format!(" {title} ");
            let rect = Rect::new(x, 2, (text.width() as u16).min(area.width - 1 - x), 1);
            line(
                frame,
                rect,
                &text,
                if self.tab == tab {
                    accent.add_modifier(Modifier::REVERSED)
                } else {
                    dim
                },
            );
            self.hits.push((rect, Hit::Tab(tab)));
            x = rect.right() + 1;
        }
        let footer_y = area.height.saturating_sub(1);
        let content = Rect::new(1, 4.min(footer_y), width, footer_y.saturating_sub(4));
        if self.tab == Tab::Configuration {
            self.text_panel(
                frame,
                content,
                " Configuration ",
                &self.configuration(),
                true,
            );
        } else if self.expanded {
            self.text_panel(frame, content, " Server details ", &self.detail(), true);
        } else {
            self.render_servers(frame, content);
        }
        let hint = if self.expanded {
            "↑↓/PgUp/PgDn Scroll · Enter/Esc Back · Tab Sections"
        } else if self.tab == Tab::Configuration {
            "↑↓/PgUp/PgDn Scroll · Tab Sections · Esc Close"
        } else if area.width < 65 {
            "↑↓ Select · Enter Details · Tab More · Esc Close"
        } else {
            "Type to search · ↑↓/PgUp/PgDn Select · Enter Details · Tab Sections · Esc Close"
        };
        line(frame, Rect::new(1, footer_y, width, 1), hint, dim);
    }
    fn render_servers(&mut self, frame: &mut Frame, mut area: Rect) {
        if area.height == 0 {
            return;
        }
        let dim = Style::default().add_modifier(Modifier::DIM);
        line(
            frame,
            Rect::new(area.x, area.y, area.width.min(8), 1),
            "Search: ",
            Style::default().fg(Color::Cyan),
        );
        let search_width = area.width.saturating_sub(8) as usize;
        if search_width > 0 {
            let cursor = self.query.text[..self.query.cursor].width();
            let offset = cursor.saturating_sub(search_width.saturating_sub(1));
            frame.render_widget(
                Paragraph::new(self.query.text.as_str()).scroll((0, offset as u16)),
                Rect::new(area.x + 8, area.y, search_width as u16, 1),
            );
            frame.set_cursor_position((area.x + 8 + (cursor - offset) as u16, area.y));
        }
        area.y += 1;
        area.height -= 1;
        if area.height == 0 {
            return;
        }
        line(
            frame,
            Rect::new(area.x, area.y, area.width, 1),
            &format!("{} of {} servers", self.matches.len(), self.servers.len()),
            dim,
        );
        area.y += 1;
        area.height -= 1;
        if area.height == 0 {
            return;
        }
        if self.matches.is_empty() {
            let message = if self.servers.is_empty() {
                "No MCP servers configured. Close and run /mcp add."
            } else {
                "No matching servers. Clear search with Ctrl+U."
            };
            self.text_panel(frame, area, " Servers ", message, false);
            return;
        }
        let (list, detail) = if area.width >= 85 {
            let w = area.width / 2;
            (
                Rect::new(area.x, area.y, w, area.height),
                Some(Rect::new(
                    area.x + w + 1,
                    area.y,
                    area.width - w - 1,
                    area.height,
                )),
            )
        } else if area.height >= 9 {
            let h = area.height * 3 / 5;
            (
                Rect::new(area.x, area.y, area.width, h),
                Some(Rect::new(area.x, area.y + h, area.width, area.height - h)),
            )
        } else {
            (area, None)
        };
        let block = Block::default().borders(Borders::ALL).title(" Servers ");
        let inner = block.inner(list);
        frame.render_widget(block, list);
        self.page_size = inner.height.saturating_sub(1).max(1) as usize;
        line(
            frame,
            Rect::new(inner.x, inner.y, inner.width, inner.height.min(1)),
            "  ID / STATUS",
            dim,
        );
        if self.selected < self.top {
            self.top = self.selected;
        }
        if self.selected >= self.top + self.page_size {
            self.top = self.selected + 1 - self.page_size;
        }
        self.top = self
            .top
            .min(self.matches.len().saturating_sub(self.page_size));
        for (row, position) in (self.top..self.matches.len())
            .take(inner.height.saturating_sub(1) as usize)
            .enumerate()
        {
            let (id, server) = &self.servers[self.matches[position]];
            let selected = position == self.selected;
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            let rect = Rect::new(inner.x, inner.y + 1 + row as u16, inner.width, 1);
            let suffix = format!(" [{}]", status(server));
            let name_width = inner.width.saturating_sub(suffix.width() as u16 + 2) as usize;
            let name = truncate(&sanitize(id), name_width);
            let padding = name_width.saturating_sub(name.width());
            let text = format!(
                "{} {}{}{}",
                if selected { "›" } else { " " },
                name,
                " ".repeat(padding),
                suffix
            );
            line(frame, rect, &text, style);
            self.hits.push((rect, Hit::Server(position)));
        }
        if let Some(rect) = detail {
            self.text_panel(
                frame,
                rect,
                " Preview · Enter for full details ",
                &self.detail(),
                false,
            );
        }
    }
    fn text_panel(
        &mut self,
        frame: &mut Frame,
        rect: Rect,
        title: &str,
        text: &str,
        scrollable: bool,
    ) {
        if rect.height == 0 || rect.width == 0 {
            return;
        }
        let block = Block::default().borders(Borders::ALL).title(title);
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let lines = wrap(text, inner.width.max(1) as usize);
        let start = if scrollable {
            self.max_scroll = lines.len().saturating_sub(inner.height as usize);
            self.scroll = self.scroll.min(self.max_scroll);
            self.page_size = inner.height.max(1) as usize;
            self.scroll
        } else {
            0
        };
        let rows: Vec<Line<'_>> = lines
            .iter()
            .skip(start)
            .take(inner.height as usize)
            .map(|text| Line::from(text.as_str()))
            .collect();
        frame.render_widget(Paragraph::new(rows), inner);
    }
}
fn status(server: &McpServerConfig) -> &'static str {
    if server.enabled {
        "enabled"
    } else {
        "disabled"
    }
}
fn line(frame: &mut Frame, rect: Rect, text: &str, style: Style) {
    frame.render_widget(Paragraph::new(text).style(style), rect);
}
fn truncate(text: &str, width: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    if text.width() <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for g in text.graphemes(true) {
        if out.width() + g.width() >= width {
            break;
        }
        out.push_str(g);
    }
    out.push('…');
    out
}
fn wrap(text: &str, width: usize) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut row = String::new();
        let mut used = 0;
        for g in line.graphemes(true) {
            let g = if g == "\t" { "    " } else { g };
            let size = g.width();
            if used + size > width && !row.is_empty() {
                lines.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push_str(g);
            used += size;
        }
        lines.push(row);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::{fixture, render};

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE), 40)
            .unwrap();
    }
    fn server(command: &str, enabled: bool) -> McpServerConfig {
        McpServerConfig {
            command: command.into(),
            enabled,
            args: vec!["--stdio".into()],
            inherit_env: vec!["PATH".into(), "SERVICE_TOKEN".into()],
            ..Default::default()
        }
    }
    #[test]
    fn browser_search_details_tabs_are_passive_and_preserve_draft() {
        let (_temp, mut app, mut rx) = fixture();
        let marker = app.paths.home.join("server-started");
        let mut alpha = server("/bin/sh", true);
        alpha.args = vec!["-c".into(), format!("touch '{}'", marker.display())];
        app.config.mcp_servers.insert("alpha".into(), alpha);
        app.config
            .mcp_servers
            .insert("beta".into(), server("/not-installed/search-server", false));
        app.paths.save(&app.config).unwrap();
        let config_before = std::fs::read(app.paths.config_file()).unwrap();
        app.editor.set("draft must survive".into());
        let revision = app.editor.revision;
        app.command("/mcp", 40).unwrap();
        let view = render(&mut app, 120, 40);
        assert!(view.contains("MCP Servers"));
        assert!(view.contains("2 servers · 1 enabled · 1 disabled · stdio only"));
        assert!(view.contains("alpha [enabled]"));
        press(&mut app, KeyCode::Down);
        assert!(render(&mut app, 120, 40).contains("beta [disabled]"));
        app.paste("SEARCH-SERVER").unwrap();
        assert!(render(&mut app, 120, 40).contains("1 of 2 servers"));
        press(&mut app, KeyCode::Enter);
        let view = render(&mut app, 100, 40);
        assert!(view.contains("Server details"));
        assert!(view.contains("Executable: /not-installed/search-server"));
        assert!(view.contains("Arguments: [\"--stdio\"]"));
        assert!(view.contains("Environment names: [\"PATH\",\"SERVICE_TOKEN\"]"));
        assert!(view.contains("Timeout: 60s"));
        assert!(view.contains("Message limit: 1048576 bytes"));
        press(&mut app, KeyCode::Esc);
        assert!(app.mcp_browser.is_some());
        press(&mut app, KeyCode::Tab);
        let view = render(&mut app, 100, 40);
        assert!(view.contains("Global configuration"));
        assert!(view.contains("/mcp edit <id>"));
        assert!(view.contains("Ask/BYPASS approval policy"));
        app.paste("ignored").unwrap();
        press(&mut app, KeyCode::BackTab);
        assert!(render(&mut app, 100, 40).contains("SEARCH-SERVER"));
        press(&mut app, KeyCode::Esc);
        assert!(app.mcp_browser.is_none());
        assert_eq!(app.editor.text, "draft must survive");
        assert_eq!(app.editor.revision, revision);
        assert_eq!(app.mode, Mode::Plan);
        assert!(app.active.is_none());
        assert!(rx.try_recv().is_err());
        assert!(!marker.exists());
        assert_eq!(
            std::fs::read(app.paths.config_file()).unwrap(),
            config_before
        );
    }
    #[test]
    fn browser_empty_no_matches_and_search_editing() {
        let (_temp, mut app, mut rx) = fixture();
        app.command("/mcp", 40).unwrap();
        let view = render(&mut app, 100, 40);
        assert!(view.contains("0 servers · 0 enabled · 0 disabled"));
        assert!(view.contains("No MCP servers configured."));
        press(&mut app, KeyCode::Enter);
        assert!(!app.mcp_browser.as_ref().unwrap().expanded);
        press(&mut app, KeyCode::Esc);
        app.config
            .mcp_servers
            .insert("demo".into(), server("/missing/demo", false));
        app.command("/mcp", 40).unwrap();
        app.paste("NOT-FOUND").unwrap();
        assert!(render(&mut app, 100, 40).contains("No matching servers."));
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert!(render(&mut app, 100, 40).contains("1 of 1 servers"));
        for ch in "DEMO".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        assert_eq!(app.mcp_browser.as_ref().unwrap().matches.len(), 1);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.mcp_browser.as_ref().unwrap().query.text, "DEMo");
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn browser_pages_resizes_scrolls_and_accepts_mouse() {
        let (_temp, mut app, mut rx) = fixture();
        for i in 0..60 {
            app.config.mcp_servers.insert(
                format!("server-{i:02}"),
                server("/missing/server", i % 2 == 0),
            );
        }
        app.command("/mcp", 40).unwrap();
        for (w, h) in [
            (120, 40),
            (48, 24),
            (48, 16),
            (24, 10),
            (8, 5),
            (4, 5),
            (3, 4),
        ] {
            render(&mut app, w, h);
        }
        render(&mut app, 48, 16);
        press(&mut app, KeyCode::PageDown);
        assert!(app.mcp_browser.as_ref().unwrap().selected > 0);
        app.key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL), 16)
            .unwrap();
        assert!(render(&mut app, 48, 16).contains("server-59"));
        let browser = app.mcp_browser.as_ref().unwrap();
        let (rect, _) = browser
            .hits
            .iter()
            .find(|(_, hit)| matches!(hit, Hit::Server(_)))
            .unwrap();
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(event, 16).unwrap();
        assert!(app.mcp_browser.as_ref().unwrap().selected < 59);
        app.mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..event
            },
            16,
        )
        .unwrap();
        press(&mut app, KeyCode::Enter);
        render(&mut app, 24, 10);
        press(&mut app, KeyCode::End);
        assert!(render(&mut app, 24, 10).contains("browser."));
        assert!(app.mcp_browser.as_ref().unwrap().scroll > 0);
        render(&mut app, 120, 40);
        assert_eq!(app.mcp_browser.as_ref().unwrap().scroll, 0);
        press(&mut app, KeyCode::Enter);
        render(&mut app, 100, 40);
        let (rect, _) = app
            .mcp_browser
            .as_ref()
            .unwrap()
            .hits
            .iter()
            .find(|(_, hit)| matches!(hit, Hit::Tab(Tab::Configuration)))
            .unwrap();
        app.mouse(
            MouseEvent {
                column: rect.x,
                row: rect.y,
                ..event
            },
            40,
        )
        .unwrap();
        assert!(render(&mut app, 100, 40).contains("Global configuration"));
        app.cancel();
        assert!(app.mcp_browser.is_none());
        assert!(!app.quit);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn browser_bounds_unicode_search_and_sanitizes_configuration() {
        let (_temp, mut app, mut rx) = fixture();
        let mut config = server("/missing/日本語\x1b[2J", true);
        config.args = vec!["e\u{301}\x1b[2J".into()];
        app.config.mcp_servers.insert("demo".into(), config);
        app.command("/mcp", 40).unwrap();
        app.paste("日本語").unwrap();
        assert_eq!(app.mcp_browser.as_ref().unwrap().matches.len(), 1);
        let view = render(&mut app, 120, 40);
        assert!(!view.contains('\x1b'));
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.mcp_browser.as_ref().unwrap().query.text, "日本");
        app.paste(&"語".repeat(2000)).unwrap();
        assert!(app.mcp_browser.as_ref().unwrap().query.text.len() <= 1024);
        let mut release = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.key(release, 40).unwrap();
        assert!(app.mcp_browser.is_some());
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert!(app.mcp_browser.is_none());
        assert!(!app.quit);
        assert!(app.editor.text.is_empty());
        assert!(rx.try_recv().is_err());
    }
}
