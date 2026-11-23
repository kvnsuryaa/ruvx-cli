//! Passive, searchable skill catalog. Never reads skill bodies or starts work.
use super::*;
use crate::skills::Skill;
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Skills,
    Locations,
    Warnings,
}
#[derive(Clone, Copy)]
enum Hit {
    Tab(Tab),
    Skill(usize),
}

pub(super) struct Browser {
    skills: Vec<Skill>,
    warnings: Vec<String>,
    locations: Vec<(PathBuf, &'static str)>,
    home: PathBuf,
    canonical_home: PathBuf,
    enabled: bool,
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
    pub(super) fn new(
        skills: Vec<Skill>,
        warnings: Vec<String>,
        locations: Vec<(PathBuf, &'static str)>,
        home: PathBuf,
        enabled: bool,
    ) -> Self {
        let matches = (0..skills.len()).collect();
        Self {
            skills,
            warnings,
            locations,
            canonical_home: home.canonicalize().unwrap_or_else(|_| home.clone()),
            home,
            enabled,
            query: Editor::default(),
            matches,
            selected: 0,
            top: 0,
            tab: Tab::Skills,
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
            .skills
            .iter()
            .enumerate()
            .filter_map(|(index, skill)| {
                (skill.name.to_lowercase().contains(&query)
                    || skill.description.to_lowercase().contains(&query))
                .then_some(index)
            })
            .collect();
        self.selected = 0;
        self.top = 0;
        self.scroll = 0;
    }
    pub(super) fn paste(&mut self, text: &str) {
        if self.tab != Tab::Skills || self.expanded {
            return;
        }
        let text = sanitize(text)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        // A search field is not a message composer; bound pasted queries.
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
            let backward =
                key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
            let tab = match (self.tab, backward) {
                (Tab::Skills, false) | (Tab::Warnings, true) => Tab::Locations,
                (Tab::Locations, false) | (Tab::Skills, true) => Tab::Warnings,
                _ => Tab::Skills,
            };
            self.change_tab(tab);
            return false;
        }
        if self.expanded || self.tab != Tab::Skills {
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
                KeyCode::Home => {
                    self.selected = 0;
                    self.scroll = 0;
                }
                KeyCode::End => {
                    self.selected = self.matches.len().saturating_sub(1);
                    self.scroll = 0;
                }
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
                    self.query.insert(&ch.to_string())
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
                if self.tab == Tab::Skills && !self.expanded {
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
                        Hit::Skill(index) if index < self.matches.len() => {
                            self.selected = index;
                            self.scroll = 0;
                        }
                        Hit::Skill(_) => {}
                    }
                }
            }
            _ => {}
        }
    }
    fn path(&self, path: &Path) -> String {
        let path = path
            .strip_prefix(&self.home)
            .or_else(|_| path.strip_prefix(&self.canonical_home))
            .map(|p| format!("~/{}", p.display()))
            .unwrap_or_else(|_| path.display().to_string());
        sanitize(&path)
    }
    fn detail(&self) -> String {
        let Some(skill) = self.matches.get(self.selected).map(|&i| &self.skills[i]) else {
            return String::new();
        };
        format!(
            "{} [{}]\n\n{}\n\nLocation: {}\n\nMetadata only. No scripts are executed.",
            skill.name,
            skill.source,
            sanitize(&skill.description),
            self.path(&skill.directory)
        )
    }
    pub(super) fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.hits.clear();
        let width = area.width.saturating_sub(2);
        let accent = Style::default().fg(Color::Cyan);
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        line(frame, Rect::new(1, 0, width, 1), "Agent Skills", bold);
        let summary = format!(
            "{} · {} skills · {} locations · {} warnings",
            if self.enabled { "Enabled" } else { "Disabled" },
            self.skills.len(),
            self.locations.len(),
            self.warnings.len()
        );
        line(frame, Rect::new(1, 1, width, 1), &summary, dim);
        let mut x = 1;
        for (tab, title) in [
            (Tab::Skills, "Skills"),
            (Tab::Locations, "Locations"),
            (Tab::Warnings, "Warnings"),
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
        if self.tab != Tab::Skills || self.expanded {
            let text = match self.tab {
                Tab::Skills => self.detail(),
                Tab::Locations => {
                    let mut text =
                        String::from("Discovery locations (lowest to highest priority)\n\n");
                    for (path, source) in &self.locations {
                        text.push_str(&format!("[{source}] {}\n\n", self.path(path)));
                    }
                    text.push_str("Project skills override custom and user skills with the same name.\n\nManage after closing this browser:\n/skills add · /skills remove\n/skills enable · /skills disable\n\nNo scripts are executed.");
                    text
                }
                Tab::Warnings => {
                    if self.warnings.is_empty() {
                        "No warnings.\n\nAll available skill locations were checked without discovery errors.".into()
                    } else {
                        self.warnings
                            .iter()
                            .enumerate()
                            .map(|(i, w)| format!("{}. {}", i + 1, sanitize(w)))
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    }
                }
            };
            let title = match self.tab {
                Tab::Skills => " Skill details ",
                Tab::Locations => " Locations ",
                Tab::Warnings => " Discovery warnings ",
            };
            self.text_panel(frame, content, title, &text, true);
        } else {
            self.render_skills(frame, content);
        }
        let hint = if self.expanded {
            "↑↓/PgUp/PgDn Scroll · Enter/Esc Back · Tab Sections"
        } else if self.tab != Tab::Skills {
            "↑↓/PgUp/PgDn Scroll · Tab Sections · Esc Close"
        } else if area.width < 65 {
            "↑↓ Select · Enter Details · Tab More · Esc Close"
        } else {
            "Type to search · ↑↓/PgUp/PgDn Select · Enter Details · Tab Sections · Esc Close"
        };
        line(frame, Rect::new(1, footer_y, width, 1), hint, dim);
    }
    fn render_skills(&mut self, frame: &mut Frame, mut area: Rect) {
        if area.height == 0 {
            return;
        }
        let dim = Style::default().add_modifier(Modifier::DIM);
        let search_width = area.width.saturating_sub(8) as usize;
        line(
            frame,
            Rect::new(area.x, area.y, area.width.min(8), 1),
            "Search: ",
            Style::default().fg(Color::Cyan),
        );
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
        let counter = format!("{} of {} skills", self.matches.len(), self.skills.len());
        line(
            frame,
            Rect::new(area.x, area.y, area.width, 1),
            &counter,
            dim,
        );
        area.y += 1;
        area.height -= 1;
        if area.height == 0 {
            return;
        }
        if self.matches.is_empty() {
            let message = if !self.enabled {
                "Skill discovery is disabled. Close and run /skills enable."
            } else if self.skills.is_empty() {
                "No skills discovered. See Locations or use /skills add."
            } else {
                "No matching skills. Clear search with Ctrl+U."
            };
            self.text_panel(frame, area, " Skills ", message, false);
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
        let block = Block::default().borders(Borders::ALL).title(" Skills ");
        let inner = block.inner(list);
        frame.render_widget(block, list);
        self.page_size = inner.height.saturating_sub(1).max(1) as usize;
        line(
            frame,
            Rect::new(inner.x, inner.y, inner.width, inner.height.min(1)),
            "  NAME / SOURCE",
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
        let visible = inner.height.saturating_sub(1) as usize;
        for (row, position) in (self.top..self.matches.len()).take(visible).enumerate() {
            let skill = &self.skills[self.matches[position]];
            let selected = position == self.selected;
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            let rect = Rect::new(inner.x, inner.y + 1 + row as u16, inner.width, 1);
            // Keep the source visible even when the skill name needs truncation.
            let source = format!(" [{}]", skill.source);
            let name_width = inner.width.saturating_sub(source.width() as u16 + 2) as usize;
            let name = truncate(&skill.name, name_width);
            let text = format!(
                "{} {:width$}{}",
                if selected { "›" } else { " " },
                name,
                source,
                width = name_width
            );
            line(frame, rect, &text, style);
            self.hits.push((rect, Hit::Skill(position)));
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
        let max = lines.len().saturating_sub(inner.height as usize);
        let start = if scrollable {
            self.max_scroll = max;
            self.scroll = self.scroll.min(max);
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
fn line(frame: &mut Frame, rect: Rect, text: &str, style: Style) {
    frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), rect);
}
fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars() {
        if out.width() + ch.to_string().width() >= width {
            break;
        }
        out.push(ch);
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

    fn write_skill(base: &Path, name: &str, description: &str) {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: {description}\n---\nNever execute this body.\n"
            ),
        )
        .unwrap();
    }
    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE), 40)
            .unwrap();
    }
    #[test]
    fn browser_search_navigation_details_tabs_and_close_are_passive_and_preserve_draft() {
        let (_temp, mut app, mut rx) = fixture();
        let home = app.paths.home.join(".agents/skills");
        write_skill(&home, "alpha", "First skill");
        write_skill(&home, "review", "Rust inspection");
        let root = app.session.as_ref().unwrap().root.clone();
        write_skill(&root.join(".agents/skills"), "zeta", "Project skill");
        app.editor.set("draft must survive".into());
        let revision = app.editor.revision;
        app.command("/skills", 40).unwrap();
        let view = render(&mut app, 100, 40);
        assert!(view.contains("Enabled · 3 skills · 3 locations · 0 warnings"));
        assert!(view.contains("First skill"));
        assert!(view.contains("~/.agents/skills/alpha"));
        press(&mut app, KeyCode::Down);
        assert!(render(&mut app, 100, 40).contains("Rust inspection"));
        app.paste("RUST").unwrap();
        let view = render(&mut app, 100, 40);
        assert!(view.contains("1 of 3 skills"));
        assert!(view.contains("review [user]"));
        press(&mut app, KeyCode::Enter);
        assert!(render(&mut app, 48, 16).contains("Skill details"));
        press(&mut app, KeyCode::Esc);
        assert!(app.skill_browser.is_some());
        press(&mut app, KeyCode::Tab);
        assert!(
            render(&mut app, 100, 40).contains("Discovery locations (lowest to highest priority)")
        );
        press(&mut app, KeyCode::Tab);
        assert!(render(&mut app, 100, 40).contains("No warnings."));
        press(&mut app, KeyCode::BackTab);
        assert_eq!(
            app.skill_browser.as_ref().unwrap().tab as u8,
            Tab::Locations as u8
        );
        press(&mut app, KeyCode::Esc);
        assert!(app.skill_browser.is_none());
        assert_eq!(app.editor.text, "draft must survive");
        assert_eq!(app.editor.revision, revision);
        assert_eq!(app.mode, Mode::Plan);
        assert!(rx.try_recv().is_err());
        assert!(app.active.is_none());
    }
    #[test]
    fn browser_empty_disabled_no_matches_and_warning_states() {
        let (_temp, mut app, mut rx) = fixture();
        app.command("/skills", 40).unwrap();
        assert!(render(&mut app, 100, 40).contains("No skills discovered."));
        press(&mut app, KeyCode::Esc);
        write_skill(&app.paths.home.join(".agents/skills"), "demo", "Example");
        app.config.skills.paths.push(app.paths.home.join("missing"));
        app.command("/skills", 40).unwrap();
        app.paste("not-found").unwrap();
        assert!(render(&mut app, 100, 40).contains("No matching skills."));
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert!(render(&mut app, 100, 40).contains("1 of 1 skills"));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        let view = render(&mut app, 100, 40);
        assert!(view.contains("1 warnings"));
        assert!(view.contains("Cannot read custom skills"));
        press(&mut app, KeyCode::Esc);
        app.command("/skills disable", 40).unwrap();
        app.command("/skills", 40).unwrap();
        let view = render(&mut app, 100, 40);
        assert!(view.contains("Disabled · 0 skills"));
        assert!(view.contains("Skill discovery is disabled."));
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn browser_pages_scrolls_resizes_and_accepts_mouse_selection() {
        let (_temp, mut app, mut rx) = fixture();
        for i in 0..60 {
            write_skill(
                &app.paths.home.join(".agents/skills"),
                &format!("skill-{i:02}"),
                "A long description to preview safely",
            );
        }
        app.command("/skills", 40).unwrap();
        for (w, h) in [(100, 40), (48, 16), (24, 10), (8, 5), (4, 5), (3, 4)] {
            render(&mut app, w, h);
        }
        render(&mut app, 48, 16);
        press(&mut app, KeyCode::PageDown);
        let selected = app.skill_browser.as_ref().unwrap().selected;
        assert!(selected > 0);
        app.key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL), 16)
            .unwrap();
        let view = render(&mut app, 48, 16);
        assert!(view.contains("skill-59"));
        let browser = app.skill_browser.as_mut().unwrap();
        let (rect, _) = browser
            .hits
            .iter()
            .find(|(_, h)| matches!(h, Hit::Skill(_)))
            .unwrap();
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(event, 16).unwrap();
        assert!(app.skill_browser.as_ref().unwrap().selected < 59);
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
        let view = render(&mut app, 24, 10);
        assert!(app.skill_browser.as_ref().unwrap().scroll > 0);
        assert!(view.contains("executed."));
        render(&mut app, 100, 40);
        assert_eq!(app.skill_browser.as_ref().unwrap().scroll, 0);
        app.cancel();
        assert!(app.skill_browser.is_none());
        assert!(!app.quit);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn browser_bounds_search_sanitizes_metadata_and_handles_unicode() {
        let (_temp, mut app, mut rx) = fixture();
        app.command("/skills", 40).unwrap();
        let browser = app.skill_browser.as_mut().unwrap();
        browser.skills.push(Skill {
            name: "demo".into(),
            description: "日本語 e\u{301}\x1b[2J safe".into(),
            directory: browser.home.join(".agents/skills/demo"),
            source: "user",
        });
        browser.filter();
        browser.paste("日本語");
        assert_eq!(browser.matches.len(), 1);
        let view = render(&mut app, 100, 40);
        assert!(view.contains("safe"));
        assert!(!view.contains('\x1b'));
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.skill_browser.as_ref().unwrap().query.text, "日本");
        app.paste(&"x".repeat(3000)).unwrap();
        assert!(app.skill_browser.as_ref().unwrap().query.text.len() <= 1024);
        let mut release = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.key(release, 40).unwrap();
        assert!(app.skill_browser.is_some());
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), 40)
            .unwrap();
        assert!(app.skill_browser.is_none());
        assert!(!app.quit);
        assert!(app.editor.text.is_empty());
        assert!(rx.try_recv().is_err());
    }
}
