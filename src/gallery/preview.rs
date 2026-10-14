use super::*;

impl Gallery {
    /// Public-image fixture, rendered through the real gallery without touching
    /// the user's capture roots, cache, watcher or background workers.
    pub(crate) fn preview(ctx: &egui::Context, paths: &[PathBuf]) -> Self {
        let (notice_tx, notice_rx) = crossbeam_channel::unbounded();
        Self {
            shots: paths
                .iter()
                .enumerate()
                .map(|(i, path)| Shot {
                    path: path.clone(),
                    taken: std::time::SystemTime::UNIX_EPOCH
                        + std::time::Duration::from_secs(1_790_899_200 - i as u64 * 3_600),
                    source: if i < 12 {
                        Source::TrontSnap
                    } else {
                        Source::ShareX
                    },
                })
                .collect(),
            filtered: (0..paths.len()).collect(),
            thumbs: ThumbCache::preview(ctx, paths),
            filter: Filter::All,
            query: String::new(),
            selected: HashSet::new(),
            sel_anchor: None,
            confirm_delete: None,
            comparison: None,
            comparison_rx: None,
            scan_rx: None,
            scan_gen: 0,
            displayed_gen: 0,
            scanning: false,
            status: None,
            watcher: None,
            notice_tx,
            notice_rx,
        }
    }
}

#[test]
fn compare_uses_only_visible_selected_stills_and_older_image_first() {
    let ctx = egui::Context::default();
    let paths = [
        PathBuf::from("new.png"),
        PathBuf::from("old.png"),
        PathBuf::from("clip.mp4"),
    ];
    let mut gallery = Gallery::preview(&ctx, &[]);
    gallery.shots = paths
        .iter()
        .enumerate()
        .map(|(index, path)| Shot {
            path: path.clone(),
            source: Source::TrontSnap,
            taken: std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(100 - index as u64),
        })
        .collect();
    gallery.filtered = vec![0, 1, 2];
    gallery.selected.extend(paths[..2].iter().cloned());
    gallery.prepare_comparison();
    let pair = gallery.comparison.take().unwrap();
    assert_eq!(pair.before, paths[1]);
    assert_eq!(pair.after, paths[0]);
    gallery.filtered = vec![0, 2];
    gallery.prepare_comparison();
    assert!(gallery.comparison.is_none());
    gallery.selected.insert(paths[2].clone());
    gallery.prepare_comparison();
    assert!(gallery.comparison.is_none());
}
