//! A live indicator for the current task, separate from transient UI notices.
use std::time::{Duration, Instant};

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAME_MILLIS: u128 = 125;

pub(super) struct Activity {
    started: Instant,
    paused_at: Option<Instant>,
    paused_duration: Duration,
    label: &'static str,
    pub turns: u32,
    pub total_tokens: Option<u64>,
    pub approval_hint: Option<&'static str>,
    pub draft_waiting: bool,
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
            paused_at: None,
            paused_duration: Duration::ZERO,
            label,
            turns: 0,
            total_tokens: None,
            approval_hint: None,
            draft_waiting: false,
            last_frame: None,
        }
    }

    pub fn pause(&mut self, now: Instant) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
            self.last_frame = None;
        }
    }

    pub fn resume(&mut self, now: Instant) {
        if let Some(paused_at) = self.paused_at.take() {
            self.paused_duration += now.saturating_duration_since(paused_at);
            self.last_frame = None;
        }
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.paused_at
            .unwrap_or(now)
            .saturating_duration_since(self.started)
            .saturating_sub(self.paused_duration)
    }

    /// Called only while work is active. Approval waits keep a frozen frame.
    pub fn tick(&mut self, now: Instant, waiting: bool) -> bool {
        let elapsed = self.elapsed(now);
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
        let elapsed = self.elapsed(now);
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
    fn elapsed_excludes_repeated_approval_waits_and_resumes_without_resetting() {
        let mut activity = Activity::default();
        let start = activity.started;
        activity.pause(start + Duration::from_secs(5));
        activity.pause(start + Duration::from_secs(10));
        let waiting = activity.text(start + Duration::from_secs(65), false, true);
        assert!(waiting.contains("Waiting for approval, 5s,"));
        activity.resume(start + Duration::from_secs(65));
        activity.resume(start + Duration::from_secs(66));
        assert_eq!(
            activity.elapsed(start + Duration::from_secs(68)),
            Duration::from_secs(8)
        );
        assert!(
            activity
                .text(start + Duration::from_secs(68), false, false)
                .contains("AI Working, 8s,")
        );
        activity.pause(start + Duration::from_secs(70));
        assert_eq!(
            activity.elapsed(start + Duration::from_secs(130)),
            Duration::from_secs(10)
        );
        activity.resume(start + Duration::from_secs(130));
        assert_eq!(
            activity.elapsed(start + Duration::from_secs(132)),
            Duration::from_secs(12)
        );
    }

    #[test]
    fn approval_lifecycle_pauses_until_answer_escape_or_cancel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for review in ["Shell approval · fixture", "MCP approval · fixture"] {
            for action in ["run", "deny", "escape", "cancel"] {
                let (_temp, mut app, _rx) = fixture();
                let cancel = app.begin_work("AI Working", "fixture");
                let (respond, mut answer) = tokio::sync::oneshot::channel();
                app.event(
                    UiEvent::Approval {
                        review: review.into(),
                        respond,
                    },
                    40,
                )
                .unwrap();
                assert!(app.activity.paused_at.is_some());
                let now = Instant::now();
                let elapsed = app.activity.elapsed(now);
                assert_eq!(app.activity.elapsed(now + Duration::from_secs(60)), elapsed);
                render(&mut app, 100, 40);
                app.editor.set("invalid answer".into());
                app.submit(40).unwrap();
                assert!(app.activity.paused_at.is_some());
                match action {
                    "run" | "deny" => {
                        app.editor.set(action.into());
                        app.submit(40).unwrap();
                        assert_eq!(answer.try_recv().unwrap(), action == "run");
                    }
                    "escape" => app
                        .key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 40)
                        .unwrap(),
                    "cancel" => app.cancel(),
                    _ => unreachable!(),
                }
                assert!(app.activity.paused_at.is_none());
                assert!(
                    app.activity
                        .elapsed(Instant::now() + Duration::from_secs(1))
                        > elapsed
                );
                assert!(matches!(app.dialog, Dialog::None));
                assert_eq!(cancel.is_cancelled(), action == "cancel");
                if matches!(action, "escape" | "cancel") {
                    assert!(matches!(
                        answer.try_recv(),
                        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
                    ));
                }
            }
        }
    }

    #[test]
    fn approval_waits_have_no_spinner_and_cancel_overrides_late_status_events() {
        let mut activity = Activity::default();
        let start = activity.started;
        activity.pause(start);
        assert!(activity.tick(start, true));
        assert!(!activity.tick(start + Duration::from_millis(900), true));
        assert!(!activity.tick(start + Duration::from_secs(60), true));
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
