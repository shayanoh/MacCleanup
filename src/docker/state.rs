//! The Docker page's state: what was last read from Docker (or why it could
//! not be), the analysis of it, the lines shown and the cursor in them.

use std::sync::Arc;

use crate::bg::{Latest, Work};
use crate::docker::analysis::{Analysis, ItemFacts, Layers, analyse};
use crate::docker::cli::{DockerCli, DockerError, Loaded};
use crate::docker::model::DockerData;
use crate::docker::view::{DLine, DockerView};
use crate::filter::Query;

/// A connection and what it read.
#[derive(Clone)]
pub struct DockerLoad {
    pub cli: DockerCli,
    pub loaded: Loaded,
    /// Our helper image exists at the current version (no build needed
    /// before a trim).
    pub vm_tools: bool,
}

pub struct DockerState {
    /// The read in progress (connect, then load); only the newest counts.
    pub job: Latest<Result<DockerLoad, DockerError>>,
    pub work: Arc<Work>,
    /// The last successful read.
    pub load: Option<DockerLoad>,
    /// Why the last read failed (cleared by a good one).
    pub error: Option<DockerError>,
    pub analysis: Analysis,
    pub view: DockerView,
    pub lines: Vec<DLine>,
    pub cursor: usize,
    pub offset: usize,
    /// A reload was asked for while one ran: run once more when it lands.
    pub stale: bool,
    /// Something read at least once (successfully or not).
    pub read_once: bool,
    /// How many lines the details pane is scrolled down, how far it can go
    /// (written by the drawing code, which knows the pane's size), and which
    /// line the scroll belongs to (it starts over for another one).
    /// The VM-disk trim was added by marking a removal (and goes away with
    /// the last one); the user marked it themselves; or they took it off
    /// while removals were marked (then marking more does not bring it back).
    pub browse: Option<crate::docker::browse::BrowseState>,
    pub trim_auto: bool,
    pub trim_declined: bool,
    pub detail_scroll: usize,
    pub detail_max: usize,
    pub detail_key: String,
}

impl Default for DockerState {
    fn default() -> Self {
        DockerState::new()
    }
}

impl DockerState {
    pub fn new() -> DockerState {
        let analysis = Analysis::default();
        DockerState {
            job: Latest::new(),
            work: Work::new(),
            load: None,
            error: None,
            view: DockerView::new(&analysis),
            analysis,
            lines: Vec::new(),
            cursor: 0,
            offset: 0,
            stale: false,
            read_once: false,
            browse: None,
            trim_auto: false,
            trim_declined: false,
            detail_scroll: 0,
            detail_max: 0,
            detail_key: String::new(),
        }
    }

    pub fn data(&self) -> Option<&DockerData> {
        self.load.as_ref().map(|l| &l.loaded.data)
    }

    pub fn layers(&self) -> Option<&Layers> {
        self.load.as_ref().map(|l| &l.loaded.layers)
    }

    pub fn selected(&self) -> Option<&DLine> {
        self.lines.get(self.cursor)
    }

    /// The facts of an item line's key (`None` for sections and layers).
    pub fn facts(&self, key: &str) -> Option<&ItemFacts> {
        self.analysis.items.get(key)
    }

    /// Take a fresh read: analyse it, keep the folds and sort the user chose
    /// (the first read opens images that have containers).
    pub fn accept(&mut self, load: DockerLoad) {
        self.analysis = analyse(&load.loaded.data, &load.loaded.layers);
        if !self.read_once {
            let sort = self.view.sort;
            self.view = DockerView::new(&self.analysis);
            self.view.sort = sort;
        }
        self.load = Some(load);
        self.error = None;
        self.read_once = true;
    }

    /// Rebuild the lines for `filter`, keeping the cursor on the same line.
    pub fn rebuild(&mut self, filter: &Query) {
        let keep = self.selected().map(|l| l.key.clone());
        self.lines = match &self.load {
            Some(l) => self
                .view
                .lines(&self.analysis, &l.loaded.data, &l.loaded.layers, filter),
            None => Vec::new(),
        };
        self.cursor = keep
            .and_then(|k| self.lines.iter().position(|l| l.key == k))
            .unwrap_or(self.cursor)
            .min(self.lines.len().saturating_sub(1));
    }
}
