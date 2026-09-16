//! Conversation-routing pure logic: per-peer message transcripts, grouping,
//! and unread counts. Deliberately UI-free — rows are plain data converted
//! to Slint messages by `main.rs`, which keeps this module unit-testable
//! without a display.

use std::collections::HashMap;

/// Identifies one conversation: the roster peer (friend `user_id` or group
/// `group_id`) plus its kind. Demo-mode rows (no real peer) share a single
/// bucket and behave exactly as before routing existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConvKey {
    pub id: i64,
    pub is_group: bool,
}

impl ConvKey {
    /// Bucket for rows that carry no real peer id (demo/mock channels).
    pub const DEMO: ConvKey = ConvKey {
        id: 0,
        is_group: false,
    };
}

/// Minutes after which the same author starts a new message group.
pub const GROUP_GAP_MIN: i64 = 5;

/// Per-conversation transcripts with grouping state and unread counters.
///
/// `T` is the row payload — `main.rs` instantiates it with the Slint
/// `Message` struct; tests use plain fixtures.
pub struct Store<T> {
    transcripts: HashMap<ConvKey, Vec<T>>,
    last_seen: HashMap<ConvKey, (String, i64)>,
    unread: HashMap<ConvKey, u32>,
    active: Option<ConvKey>,
}

impl<T: Clone + HasRowMeta> Store<T> {
    pub fn new() -> Self {
        Store {
            transcripts: HashMap::new(),
            last_seen: HashMap::new(),
            unread: HashMap::new(),
            active: None,
        }
    }

    /// Append `row` to `key`'s transcript, computing grouping from that
    /// conversation's last (author, minute) — the global-transcript rule
    /// applied per conversation. Rows landing in a non-active conversation
    /// bump its unread count. Returns whether the row landed in the
    /// currently active conversation, so the caller knows whether it must
    /// also push into the displayed Slint model.
    pub fn push(&mut self, key: ConvKey, row: &mut T, author: &str, minute: i64) -> bool {
        let grouped = match self.last_seen.get(&key) {
            Some((last_author, last_minute)) => {
                last_author == author && (0..=GROUP_GAP_MIN).contains(&(minute - *last_minute))
            }
            None => false,
        };
        self.last_seen.insert(key, (author.to_string(), minute));
        row.set_grouped(grouped);
        self.transcripts.entry(key).or_default().push(row.clone());
        let landed = self.active == Some(key);
        if !landed {
            *self.unread.entry(key).or_insert(0) += 1;
        }
        landed
    }

    /// Rows of one conversation, oldest first.
    pub fn entries(&self, key: ConvKey) -> &[T] {
        self.transcripts.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Unread count for `key` (0 = none).
    pub fn unread(&self, key: ConvKey) -> u32 {
        self.unread.get(&key).copied().unwrap_or(0)
    }

    /// Mark `key` as read (after its transcript is displayed).
    pub fn clear_unread(&mut self, key: ConvKey) {
        self.unread.insert(key, 0);
    }

    /// Switch the active conversation (does not touch unread — clearing is
    /// explicit via [`Store::clear_unread`]).
    pub fn set_active(&mut self, key: ConvKey) {
        self.active = Some(key);
    }

    pub fn active(&self) -> Option<ConvKey> {
        self.active
    }

    /// Mutable access across all conversations: send-retry outcomes arrive
    /// for whatever conversation the user has since switched away from.
    pub fn rows_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.transcripts
            .values_mut()
            .flat_map(|bucket| bucket.iter_mut())
    }
}

impl<T: Clone + HasRowMeta> Default for Store<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Bridge between the store's grouping rule and the caller's row type.
pub trait HasRowMeta {
    fn set_grouped(&mut self, grouped: bool);
}
