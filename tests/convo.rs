//! Behaviour tests for `src/convo.rs` (conversation routing store), seen as
//! a consumer would. gugu has no lib target, so the module is compiled
//! straight into this test crate via `#[path]` — same mechanism as
//! `tests/stickers.rs`.

#[path = "../src/convo.rs"]
mod convo;

use convo::{ConvKey, HasRowMeta, Store, GROUP_GAP_MIN};

/// Test-only row fixture: a plain payload that implements [`HasRowMeta`],
/// standing in for the Slint `Message` that `main.rs` uses as the store's
/// row type. No sticker/GIF concerns keep the tests free of pixels.
#[derive(Debug, Clone)]
struct Row {
    body: String,
    grouped: bool,
    uid: i32,
    client_id: i64,
}

/// A row landing in the DEMO bucket (no real peer) is never unread: the
/// demo transcript mirrors the pre-routing global behaviour.
#[test]
fn demo_bucket_never_unread() {
    let mut store: Store<Row> = Store::new();
    store.set_active(convo::ConvKey::DEMO);
    let mut a = row("demo", 1);
    assert!(store.push(convo::ConvKey::DEMO, &mut a, "you", 10));
    assert_eq!(store.unread(convo::ConvKey::DEMO), 0);
}

fn row(body: &str, uid: i32) -> Row {
    Row {
        body: body.into(),
        grouped: false,
        uid,
        client_id: 0,
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
    let mut a = row("hello", 1);
    let mut b = row("hi", 2);
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
    let mut r1 = row("first", 1);
    let mut r2 = row("second", 2);
    let mut r3 = row("later", 3);
    let mut r4 = row("other author", 4);
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
    let mut i1 = row("inbox 1", 1);
    let mut i2 = row("inbox 2", 2);
    store.push(friend(2), &mut i1, "eve", 10);
    store.push(friend(2), &mut i2, "eve", 11);
    assert_eq!(store.unread(friend(2)), 2);
    // Active conversation pushes never count as unread.
    let mut a = row("to active", 3);
    assert!(store.push(friend(1), &mut a, "me", 12));
    assert_eq!(store.unread(friend(1)), 0);
    store.set_active(friend(2));
    store.clear_unread(friend(2));
    assert_eq!(store.unread(friend(2)), 0);
    let mut i3 = row("after open", 4);
    store.push(friend(2), &mut i3, "eve", 13);
    // key == active → suppressed, not accumulated.
    assert_eq!(store.unread(friend(2)), 0);
    assert_eq!(store.active(), Some(friend(2)));
}

#[test]
fn rows_mut_spans_conversations() {
    let mut store: Store<Row> = Store::new();
    store.set_active(friend(1));
    let mut a = row("outgoing", 1);
    let mut b = row("to other conv", 2);
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
