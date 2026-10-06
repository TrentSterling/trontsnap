use super::*;

impl Gallery {
    /// Public-image fixture, rendered through the real gallery without touching
    /// the user's capture roots, cache, watcher or background workers.
    pub(crate) fn preview(ctx: &egui::Context, paths: &[PathBuf]) -> Self {
        let (notice_tx, notice_rx) = crossbeam_channel::unbounded();
        let (del_tx, del_rx) = crossbeam_channel::unbounded();
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
            scan_rx: None,
            scan_gen: 0,
            displayed_gen: 0,
            scanning: false,
            status: None,
            watcher: None,
            notice_tx,
            notice_rx,
            ctx: ctx.clone(),
            del_tx,
            del_rx,
            deleting: false,
            delete_queue: Vec::new(),
            copy_tx: crossbeam_channel::unbounded().0, // fixture: nothing is ever copied
            delete_tally: (0, 0),
            search_active: false,
            has_sharex: true,
            trace: false,
            last_trace: String::new(),
        }
    }
}
