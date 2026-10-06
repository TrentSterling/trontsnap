// Gallery state tests that need no GUI: the delete/ghost bookkeeping that used to
// leave blank tiles behind and break Ctrl+A + Delete.

use super::*;

fn gallery(paths: &[&str]) -> Gallery {
    let mut g = Gallery::preview(&egui::Context::default(), &[]);
    let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
    for (i, p) in paths.iter().enumerate() {
        g.shots.push(Shot {
            path: PathBuf::from(p),
            taken: t0 - Duration::from_secs(i as u64 * 60),
            source: Source::TrontSnap,
        });
    }
    g.rebuild_filtered();
    g
}

/// filtered must index live shots, in range, with no dupes.
fn assert_consistent(g: &Gallery) {
    assert!(g.filtered.iter().all(|&i| i < g.shots.len()));
    let uniq: HashSet<_> = g.filtered.iter().collect();
    assert_eq!(uniq.len(), g.filtered.len());
    assert!(g.selected.iter().all(|p| g.shots.iter().any(|s| &s.path == p)), "selection holds a path with no shot");
}

#[test]
fn gone_path_leaves_no_ghost() {
    let mut g = gallery(&["/x/a.png", "/x/b.png", "/x/c.png"]);
    g.selected.insert("/x/b.png".into());
    g.sel_anchor = Some("/x/b.png".into());
    assert!(g.drop_path(Path::new("/x/b.png")));
    g.rebuild_filtered();
    assert_eq!(g.shots.len(), 2);
    assert_eq!(g.filtered.len(), 2);
    assert!(g.selected.is_empty());
    assert!(g.sel_anchor.is_none());
    assert_consistent(&g);
    // a second Gone for the same path (our own delete + the watcher's) is harmless
    assert!(!g.drop_path(Path::new("/x/b.png")));
}

#[test]
fn insert_shot_ignores_a_vanished_file() {
    let mut g = gallery(&["/x/a.png"]);
    g.insert_shot("/definitely/not/here.png".into());
    assert_eq!(g.shots.len(), 1, "a missing file must not become a now()-stamped ghost");
}

#[test]
fn insert_shot_adds_a_real_file_and_drops_it_when_it_disappears() {
    let dir = std::env::temp_dir().join(format!("trontsnap-gtest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("new.png");
    std::fs::write(&f, b"x").unwrap();
    let mut g = gallery(&["/x/a.png"]);
    g.insert_shot(f.clone());
    assert_eq!(g.shots.len(), 2);
    std::fs::remove_file(&f).unwrap();
    g.insert_shot(f.clone()); // the stale Upsert that raced the delete
    assert_eq!(g.shots.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_progress_drops_only_what_left_the_disk() {
    let mut g = gallery(&["/x/a.png", "/x/b.png", "/x/c.png"]);
    for p in ["/x/a.png", "/x/b.png", "/x/c.png"] {
        g.selected.insert(p.into());
    }
    g.sel_anchor = Some("/x/a.png".into());
    g.deleting = true;
    g.del_tx
        .send(DeleteMsg::Removed { paths: vec!["/x/a.png".into(), "/x/b.png".into()], done: 3, total: 3 })
        .unwrap();
    g.del_tx.send(DeleteMsg::Done { moved: 2, failed: 1 }).unwrap();
    g.poll_delete();
    assert_eq!(g.shots.len(), 1);
    assert_eq!(g.shots[0].path, PathBuf::from("/x/c.png"), "the file that failed to trash stays");
    assert_eq!(g.selected.len(), 1);
    assert!(g.sel_anchor.is_none());
    assert!(!g.deleting);
    assert!(g.status.as_ref().unwrap().0.contains("failed"));
    assert_consistent(&g);
}

#[test]
fn a_second_delete_during_a_batch_is_queued_and_summarised_once() {
    let mut g = gallery(&["/x/a.png", "/x/b.png"]);
    g.deleting = true; // a batch is already moving
    g.delete_many(vec!["/x/b.png".into()]);
    assert_eq!(g.delete_queue, vec![PathBuf::from("/x/b.png")], "queued, not refused and not lost");
    assert!(g.status.as_ref().unwrap().0.contains("1 more queued"));
    g.del_tx.send(DeleteMsg::Removed { paths: vec!["/x/a.png".into()], done: 1, total: 1 }).unwrap();
    g.del_tx.send(DeleteMsg::Done { moved: 1, failed: 0 }).unwrap();
    g.poll_delete();
    assert!(g.deleting, "the queued batch started right after the first finished");
    assert!(g.delete_queue.is_empty());
    assert_eq!(g.delete_tally, (1, 0), "the first batch's count is carried, not reported alone");
    // /x/b.png never existed: the queued batch finds it already gone and ends with ONE summary.
    for _ in 0..100 {
        g.poll_delete();
        if !g.deleting {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!g.deleting);
    assert!(g.status.as_ref().unwrap().0.starts_with("Moved 1 to the "), "{:?}", g.status);
    assert!(g.shots.is_empty());
    assert_eq!(g.delete_tally, (0, 0));
}

#[test]
fn video_label_follows_the_extension() {
    assert_eq!(video_label(Path::new("/x/a.mp4")), "MP4");
    assert_eq!(video_label(Path::new("/x/a.webm")), "WEBM");
    assert_eq!(video_label(Path::new("/x/noext")), "VIDEO");
}
