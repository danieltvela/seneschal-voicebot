/// Manages when it's safe to announce agent task results without interrupting
/// the user or colliding with in-flight audio/turns.
///
/// Blocking logic (per issue #168):
/// - Blocked if: user is speaking, a turn is pending, or audio is queued
///   (not yet playing)
/// - NOT blocked just by having audio in playback (can announce after it
///   finishes)
///
/// Thread-safe usage: wrap in `Arc<Mutex<AnnouncementWindow>>`.
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct PendingAnnouncement {
    pub task: String,
    pub result: String,
}

#[derive(Debug)]
pub struct AnnouncementWindow {
    /// True while the user is actively speaking (VAD SpeechStart … SpeechEnd).
    user_speaking: bool,
    /// True while the LLM turn is in-flight (TranscriptReady → LLMResponseDone).
    turn_pending: bool,
    /// Number of audio responses that have been queued (SentenceReady received
    /// by TTS task) but not yet started playback.
    audio_queued_count: usize,
    /// Number of audio responses currently being played by the audio output.
    playing_count: usize,
    /// FIFO queue of agent results waiting to be announced.
    pending: VecDeque<PendingAnnouncement>,
}

impl Default for AnnouncementWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl AnnouncementWindow {
    /// Create an empty window with no pending announcements.
    pub fn new() -> Self {
        Self {
            user_speaking: false,
            turn_pending: false,
            audio_queued_count: 0,
            playing_count: 0,
            pending: VecDeque::new(),
        }
    }

    // ── Turn lifecycle ────────────────────────────────────────────────────────

    /// Mark the start of a voice turn (transcript sent to the LLM pipeline).
    pub fn begin_turn(&mut self) {
        self.turn_pending = true;
    }

    /// Mark the end of a voice turn (LLM response completed, pipeline back to
    /// Idle).
    pub fn end_turn(&mut self) {
        self.turn_pending = false;
    }

    // ── User speech ───────────────────────────────────────────────────────────

    /// Update whether the user is currently speaking.
    /// Called on `SpeechStart` (true) and `SpeechEnd` (false) from VAD.
    pub fn set_user_speaking(&mut self, speaking: bool) {
        self.user_speaking = speaking;
    }

    // ── Audio lifecycle tracking ──────────────────────────────────────────────

    /// Register that a new audio response has been queued for playback
    /// (SentenceReady received by TTS task, before synthesis/playback starts).
    pub fn queue_audio(&mut self) {
        self.audio_queued_count += 1;
    }

    /// Move one audio response from "queued" to "playing" state.
    /// Called when the TTS task spawns the actual playback.
    pub fn start_playback(&mut self) {
        if self.audio_queued_count > 0 {
            self.audio_queued_count -= 1;
        }
        self.playing_count += 1;
    }

    /// Mark one playing response as finished.
    /// Called when playback completes (normal or barge-in cancellation).
    pub fn finish_playback(&mut self) {
        if self.playing_count > 0 {
            self.playing_count -= 1;
        }
    }

    /// True if there is at least one audio response currently playing.
    pub fn is_playing(&self) -> bool {
        self.playing_count > 0
    }

    // ── Blocking logic ────────────────────────────────────────────────────────

    /// True when it is NOT safe to announce a new agent result.
    ///
    /// An announcement is blocked when:
    /// - the user is speaking,
    /// - a turn is pending (LLM is generating), or
    /// - audio is queued but not yet playing.
    ///
    /// Audio that is already playing does NOT block — the announcement can
    /// be delivered after playback finishes.
    pub fn is_blocked(&self) -> bool {
        self.user_speaking || self.turn_pending || self.audio_queued_count > 0
    }

    // ── Announcement queue ────────────────────────────────────────────────────

    /// Enqueue an agent task result to be announced when safe.
    pub fn queue_announcement(&mut self, task: String, result: String) {
        self.pending.push_back(PendingAnnouncement { task, result });
    }

    /// Try to pop the next announcement from the queue.
    ///
    /// Returns `None` if `is_blocked()` is true or if the queue is empty.
    /// Returns `Some(announcement)` when it is safe to announce.
    pub fn pop_announcement(&mut self) -> Option<PendingAnnouncement> {
        if self.is_blocked() {
            return None;
        }
        self.pending.pop_front()
    }

    /// True if there are announcements waiting in the queue.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Interrupt: clear all pending announcements and reset audio-queued
    /// count. Does NOT affect `playing_count` (ongoing playback isn't
    /// cancelled by us), `user_speaking`, or `turn_pending`.
    pub fn interrupt(&mut self) {
        self.pending.clear();
        self.audio_queued_count = 0;
    }

    /// Reset all state back to defaults. Use with care (e.g. shutdown).
    pub fn reset(&mut self) {
        self.user_speaking = false;
        self.turn_pending = false;
        self.audio_queued_count = 0;
        self.playing_count = 0;
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_is_not_blocked() {
        let w = AnnouncementWindow::new();
        assert!(!w.is_blocked());
    }

    #[test]
    fn blocked_when_user_speaking() {
        let mut w = AnnouncementWindow::new();
        w.set_user_speaking(true);
        assert!(w.is_blocked());
    }

    #[test]
    fn blocked_when_turn_pending() {
        let mut w = AnnouncementWindow::new();
        w.begin_turn();
        assert!(w.is_blocked());
    }

    #[test]
    fn blocked_when_audio_queued() {
        let mut w = AnnouncementWindow::new();
        w.queue_audio();
        assert!(w.is_blocked());
    }

    #[test]
    fn not_blocked_when_only_playing() {
        let mut w = AnnouncementWindow::new();
        w.start_playback(); // playing_count=1, audio_queued_count=0
        assert!(!w.is_blocked());
    }

    #[test]
    fn pop_returns_none_when_blocked() {
        let mut w = AnnouncementWindow::new();
        w.queue_announcement("test".into(), "result".into());
        w.set_user_speaking(true);
        assert!(w.pop_announcement().is_none());
    }

    #[test]
    fn pop_returns_some_when_not_blocked() {
        let mut w = AnnouncementWindow::new();
        w.queue_announcement("test".into(), "result".into());
        let a = w.pop_announcement().unwrap();
        assert_eq!(a.task, "test");
        assert_eq!(a.result, "result");
    }

    #[test]
    fn pop_returns_none_when_queue_empty() {
        let mut w = AnnouncementWindow::new();
        assert!(w.pop_announcement().is_none());
    }

    #[test]
    fn interrupt_clears_pending_and_audio_queued() {
        let mut w = AnnouncementWindow::new();
        w.queue_announcement("t1".into(), "r1".into());
        w.queue_audio();
        w.queue_audio();
        w.start_playback(); // playing_count=1
        assert!(w.has_pending());
        assert_eq!(w.audio_queued_count, 1); // 2 queued, 1 moved to playing

        w.interrupt();
        assert!(!w.has_pending());
        assert_eq!(w.audio_queued_count, 0);
        assert_eq!(w.playing_count, 1); // not cleared by interrupt
    }

    #[test]
    fn reset_clears_everything() {
        let mut w = AnnouncementWindow::new();
        w.set_user_speaking(true);
        w.begin_turn();
        w.queue_audio();
        w.start_playback();
        w.queue_announcement("t".into(), "r".into());

        w.reset();
        assert!(!w.is_blocked());
        assert!(!w.is_playing());
        assert!(!w.has_pending());
    }

    #[test]
    fn end_turn_unblocks() {
        let mut w = AnnouncementWindow::new();
        w.begin_turn();
        assert!(w.is_blocked());
        w.end_turn();
        assert!(!w.is_blocked());
    }

    #[test]
    fn queue_audio_then_start_playback_moves_count() {
        let mut w = AnnouncementWindow::new();
        w.queue_audio();
        w.queue_audio();
        assert_eq!(w.audio_queued_count, 2);
        assert_eq!(w.playing_count, 0);
        assert!(w.is_blocked());

        w.start_playback();
        assert_eq!(w.audio_queued_count, 1);
        assert_eq!(w.playing_count, 1);
        assert!(w.is_blocked()); // still blocked because audio_queued > 0

        w.start_playback();
        assert_eq!(w.audio_queued_count, 0);
        assert_eq!(w.playing_count, 2);
        assert!(!w.is_blocked()); // no longer blocked — only playing
    }

    #[test]
    fn finish_playback_decrements() {
        let mut w = AnnouncementWindow::new();
        w.start_playback();
        w.start_playback();
        assert_eq!(w.playing_count, 2);
        assert!(w.is_playing());

        w.finish_playback();
        assert_eq!(w.playing_count, 1);
        assert!(w.is_playing());

        w.finish_playback();
        assert_eq!(w.playing_count, 0);
        assert!(!w.is_playing());
    }

    #[test]
    fn fifo_order_is_preserved() {
        let mut w = AnnouncementWindow::new();
        w.queue_announcement("a".into(), "ra".into());
        w.queue_announcement("b".into(), "rb".into());
        w.queue_announcement("c".into(), "rc".into());

        assert_eq!(w.pop_announcement().unwrap().task, "a");
        assert_eq!(w.pop_announcement().unwrap().task, "b");
        assert_eq!(w.pop_announcement().unwrap().task, "c");
        assert!(w.pop_announcement().is_none());
    }
}
