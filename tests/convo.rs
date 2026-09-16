//! Behaviour tests for `src/convo.rs` (conversation routing store), seen as
//! a consumer would. gugu has no lib target, so the module is compiled
//! straight into this test crate via `#[path]` — same mechanism as
//! `tests/stickers.rs`.

#[path = "../src/convo.rs"]
mod convo;

use convo::{ConvKey, HasRowMeta, Row, Store, GROUP_GAP_MIN};

/// A sticker-less row fixture: `None` sticker keeps the tests free of any
/// Slint pixel concerns; the type itself just flows through the store.
fn row(author: &str, body: &str, minute: i64, uid: i32) -> Row {
    Row {
        author: author.into(),
        body: body.into(),
        minute,
        grouped: false,
        uid,
        client_id: 0,
        sticker: None,
    }
}

impl HasRowMeta for Row {
    fn set_grouped(&mut self, grouped: bool) {
        self.grouped = grouped;
    }
}

fn friend(id: i64) -> ConvKey {
    ConvKey {
        id,
        is_group: false,
    }
}

#[test]
fn push_routes_by_key() {
    let mut store: Store<Row> = Store::new();
    store.set_active(friend(1));
    let mut a = row("alice", "hello", 10, 1);
    let mut b = row("bob", "hi", 11, 2);
    assert!(store.push(friend(1), &mut a, "alice", 10));
    assert!(!store.push(friend(2), &mut b, "bob", 11));
    assert_eq!(store.entries(friend(1)).len(), 1);
    assert_eq!(store.entries(friend(2)).len(), 1);
    assert_eq!(store.entries(friend(1))[0].body, "hello");
    assert_eq!(store.entries(friend(2))[0].body, "hi");
}

#[test]
fn push_groups_within_gap() {
    let mut store: Store<Row> = Store::new();
    store.set_active(friend(1));
    let mut r1 = row("alice", "first", 10, 1);
    let mut r2 = row("alice", "second", 13, 2); // +3 min, within gap
    let mut r3 = row("alice", "later", 20, 3); // +7 min, beyond gap
    let mut r4 = row("bob", "other author", 21, 4);
    store.push(friend(1), &mut r1, "alice", 10);
    store.push(friend(1), &mut r2, "alice", 13);
    store.push(friend(1), &mut r3, "alice", 20);
    store.push(friend(1), &mut r4, "bob", 21);
    let rows = store.entries(friend(1));
    assert!(!rows[0].grouped);
    assert!(rows[1].grouped);
    assert!(!rows[2].grouped);
    assert!(!rows[3].grouped);
    // Constant pins the Discord/Revolt grouping window.
    assert_eq!(GROUP_GAP_MIN, 5);
}

#[test]
fn unread_counts_and_active_suppression() {
    let mut store: Store<Row> = Store::new();
    store.set_active(friend(1));
    let mut i1 = row("eve", "inbox 1", 10, 1);
    let mut i2 = row("eve", "inbox 2", 11, 2);
    store.push(friend(2), &mut i1, "eve", 10);
    store.push(friend(2), &mut i2, "eve", 11);
    assert_eq!(store.unread(friend(2)), 2);
    // Active conversation pushes never count as unread.
    let mut a = row("me", "to active", 12, 3);
    assert!(store.push(friend(1), &mut a, "me", 12));
    assert_eq!(store.unread(friend(1)), 0);
    store.set_active(friend(2));
    store.clear_unread(friend(2));
    assert_eq!(store.unread(friend(2)), 0);
    let mut i3 = row("eve", "after open", 13, 4);
    store.push(friend(2), &mut i3, "eve", 13);
    // key == active → suppressed, not accumulated.
    assert_eq!(store.unread(friend(2)), 0);
}

#[test]
fn rows_mut_spans_conversations() {
    let mut store: Store<Row> = Store::new();
    store.set_active(friend(1));
    let mut a = row("me", "outgoing", 10, 1);
    let mut b = row("me", "to other conv", 11, 2);
    store.push(friend(1), &mut a, "me", 10);
    store.push(friend(2), &mut b, "me", 11);
    // Send-retry outcome: flip status by uid across conversations.
    for r in store.rows_mut() {
        if r.uid == 2 {
            r.client_id = 99;
        }
    }
    assert_eq!(store.entries(friend(1))[0].client_id, 0);
    assert_eq!(store.entries(friend(2))[0].client_id, 99);
}
