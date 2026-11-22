//! A live indicator for the current task, separate from transient UI notices.
use std::time::{Duration, Instant};

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAME_MILLIS: u128 = 125;

pub(super) struct Activity {
    started: Instant,
    label: &'static str,
    pub turns: u32,
    pub total_tokens: Option<u64>,
    pub approval_hint: Option<&'static str>,
    last_frame: Option<u128>,
}

impl Default for Activity {
    fn default() -> Self {
        Self::new("AI Working")
    }
}

impl Activity {
    pub fn new(label: &'static str) -> Self {
        Self {
            started: Instant::now(),
            label,
            turns: 0,
            total_tokens: None,
            approval_hint: None,
            last_frame: None,
        }
    }

    /// Called only while work is active. Approval waits update once per second.
    pub fn tick(&mut self, now: Instant, waiting: bool) -> bool {
        let elapsed = now.saturating_duration_since(self.started);
        let frame = if waiting {
            elapsed.as_secs() as u128
        } else {
            elapsed.as_millis() / FRAME_MILLIS
        };
        let changed = self.last_frame != Some(frame);
        self.last_frame = Some(frame);
        changed
    }

    pub fn text(&self, now: Instant, canceled: bool, waiting: bool) -> String {
        let elapsed = now.saturating_duration_since(self.started);
        let duration = duration(elapsed);
        let frame = FRAMES[(elapsed.as_millis() / FRAME_MILLIS % FRAMES.len() as u128) as usize];
        let (frame, label) = if canceled {
            (frame, "Canceling")
        } else if waiting {
            ('?', "Waiting for approval")
        } else {
            (frame, self.label)
        };
        let total = self
            .total_tokens
            .map_or_else(|| "—".into(), |n| n.to_string());
        let turn_label = if self.turns == 1 { "turn" } else { "turns" };
        format!(
            "{frame} {label}, {duration}, {} {turn_label}, {total} total tokens",
            self.turns
        )
    }
}

fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

impl super::App {
    pub(super) fn begin_work(
        &mut self,
        label: &'static str,
        detail: &str,
    ) -> tokio_util::sync::CancellationToken {
        let cancel = tokio_util::sync::CancellationToken::new();
        self.active = Some(cancel.clone());
        self.status = detail.into();
        self.activity = Activity::new(label);
        cancel
    }

    pub(super) fn finish_work(&mut self) {
        self.active = None;
        self.activity = Activity::default();
        self.status = "ready".into();
    }

    pub(super) fn activity_status(&self) -> Option<String> {
        let cancel = self.active.as_ref()?;
        Some(self.activity.text(
            Instant::now(),
            cancel.is_cancelled(),
            matches!(self.dialog, super::Dialog::Approval { .. }),
        ))
    }

    pub(super) fn activity_tick(&mut self, now: Instant) -> bool {
        let Some(cancel) = self.active.as_ref() else {
            return false;
        };
        self.activity.tick(
            now,
            !cancel.is_cancelled() && matches!(self.dialog, super::Dialog::Approval { .. }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::UiEvent,
        ui::{
            Dialog,
            tests::{fixture, render},
        },
    };

    #[test]
    fn active_status_survives_notices_and_turn_end_until_run_finishes() {
        let (_temp, mut app, _rx) = fixture();
        app.editor.set("unsent draft".into());
        app.begin_work("AI Working", "turn 1/30");
        app.event(
            UiEvent::RunProgress {
                turns: 1,
                total_tokens: None,
            },
            40,
        )
        .unwrap();
        assert!(render(&mut app, 100, 40).contains("AI Working"));
        app.event(UiEvent::ClipboardFinished(Ok(None)), 40).unwrap();
        let view = render(&mut app, 100, 40);
        assert!(view.contains("1 turn, — total tokens"));
        assert!(!view.contains("Waiting for AI response"));
        assert!(!view.contains("Copied to clipboard"));
        app.git_branch = Some("feature/updated".into());
        assert!(render(&mut app, 74, 28).contains("1 turn, — total tokens"));
        app.git_branch = None;
        app.event(UiEvent::Text("response".into()), 40).unwrap();
        assert!(!render(&mut app, 100, 40).contains("Receiving AI response"));
        app.event(UiEvent::TurnEnded, 40).unwrap();
        assert!(render(&mut app, 100, 40).contains("AI Working"));
        app.event(UiEvent::Status("Running mcp_call_tool…".into()), 40)
            .unwrap();
        app.event(
            UiEvent::RunProgress {
                turns: 2,
                total_tokens: Some(1530),
            },
            40,
        )
        .unwrap();
        let view = render(&mut app, 100, 40);
        assert!(view.contains("2 turns, 1530 total tokens"));
        assert!(!view.contains("Running mcp_call_tool"));
        for (width, height) in [(100, 40), (48, 16), (24, 10), (8, 5)] {
            let view = render(&mut app, width, height);
            if width >= 24 {
                let position = view.find("AI Working").unwrap();
                assert!(position > usize::from(width) * usize::from(height / 2));
                assert!(position < view.find("unsent draft").unwrap());
                assert!(app.frame.transcript.bottom() < app.frame.composer.y - 1);
            }
        }
        assert_eq!(app.editor.text, "unsent draft");
        let session = app.session.take().unwrap();
        app.event(
            UiEvent::RunFinished {
                session: Box::new(session),
                error: None,
            },
            40,
        )
        .unwrap();
        assert!(app.activity_status().is_none());
        assert!(!app.activity_tick(Instant::now() + Duration::from_secs(5)));
        assert!(!render(&mut app, 100, 40).contains("AI Working"));
        assert_eq!(app.status, "ready");
        assert_eq!(app.editor.text, "unsent draft");
        app.begin_work("AI Working", "New run");
        let status = app.activity_status().unwrap();
        assert!(status.contains("0 turns, — total tokens"));
        assert!(!status.contains("1530"));
    }

    #[test]
    fn approval_and_cancellation_are_distinct_and_failure_stops_animation() {
        let (_temp, mut app, _rx) = fixture();
        let cancel = app.begin_work("AI Working", "Running mcp_list_tools…");
        let (respond, mut answer) = tokio::sync::oneshot::channel();
        app.event(
            UiEvent::Approval {
                review: "MCP approval · fixture".into(),
                respond,
            },
            40,
        )
        .unwrap();
        let view = render(&mut app, 100, 40);
        assert!(view.contains("Waiting for approval"));
        assert!(view.contains("MCP approval"));
        assert!(!view.contains("AI Working"));
        app.overlay.as_mut().unwrap().view.bottom_seen = false;
        app.editor.set("run".into());
        app.submit(40).unwrap();
        assert!(answer.try_recv().is_err());
        assert!(render(&mut app, 100, 40).contains("Scroll down"));
        app.editor.set("deny".into());
        app.submit(40).unwrap();
        assert!(!answer.try_recv().unwrap());
        assert!(render(&mut app, 100, 40).contains("AI Working"));
        app.cancel();
        assert!(cancel.is_cancelled());
        app.event(UiEvent::Status("Late tool status".into()), 40)
            .unwrap();
        let view = render(&mut app, 100, 40);
        assert!(view.contains("Canceling"));
        assert!(!view.contains("Late tool status"));
        let session = app.session.take().unwrap();
        app.event(
            UiEvent::RunFinished {
                session: Box::new(session),
                error: Some("Run canceled".into()),
            },
            40,
        )
        .unwrap();
        assert!(matches!(app.dialog, Dialog::None));
        assert!(app.activity_status().is_none());
        assert!(!app.activity_tick(Instant::now() + Duration::from_secs(5)));
        let view = render(&mut app, 100, 40);
        assert!(view.contains("Run canceled"));
        assert!(!view.contains("Canceling"));
    }

    #[test]
    fn progress_remains_visible_in_paste_editor_and_idle_never_animates() {
        let (_temp, mut app, _rx) = fixture();
        assert!(!app.activity_tick(Instant::now()));
        app.paste(&"draft line\n".repeat(20)).unwrap();
        app.open_paste(app.editor.pastes[0].id).unwrap();
        app.begin_work("AI Working", "Waiting for response");
        assert!(app.activity_tick(Instant::now()));
        let view = render(&mut app, 100, 40);
        assert!(view.contains("Edit Paste"));
        assert!(view.find("AI Working").unwrap() > 100 * 20);
        app.finish_work();
        assert!(!render(&mut app, 100, 40).contains("AI Working"));
        assert!(!app.activity_tick(Instant::now() + Duration::from_secs(10)));
    }

    #[test]
    fn silent_work_animates_on_a_bounded_clock_and_shows_elapsed_time() {
        let mut activity = Activity::new("AI Working");
        activity.turns = 2;
        activity.total_tokens = Some(1530);
        let start = activity.started;
        assert!(activity.tick(start, false));
        assert!(!activity.tick(start + Duration::from_millis(124), false));
        assert!(activity.tick(start + Duration::from_millis(125), false));
        let first = activity.text(start, false, false);
        let second = activity.text(start + Duration::from_millis(125), false, false);
        assert_ne!(first.chars().next(), second.chars().next());
        assert_eq!(second, "⠙ AI Working, 0s, 2 turns, 1530 total tokens");
        assert!(
            activity
                .text(start + Duration::from_secs(65), false, false)
                .contains("1m 05s")
        );
    }

    #[test]
    fn approval_waits_have_no_spinner_and_cancel_overrides_late_status_events() {
        let mut activity = Activity::default();
        let start = activity.started;
        assert!(activity.tick(start, true));
        assert!(!activity.tick(start + Duration::from_millis(900), true));
        assert!(activity.tick(start + Duration::from_secs(1), true));
        let waiting = activity.text(start, false, true);
        assert_eq!(
            waiting,
            "? Waiting for approval, 0s, 0 turns, — total tokens"
        );
        assert!(!waiting.contains("AI Working"));
        let canceled = activity.text(start, true, true);
        assert!(canceled.contains("Canceling"));
        assert!(!canceled.contains("Running a late tool"));
    }
}
