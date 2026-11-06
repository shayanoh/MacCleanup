//! Looking inside an image or volume, read-only. A volume is listed by a
//! throwaway container of our helper image with the volume mounted
//! read-only and no network; an image by running only its own `sh` with our listing script, also
//! without network and with a read-only root. Nothing here can change what
//! it looks at. The first time a volume is opened the helper image is built
//! (Docker pulls busybox once if it is not there). A folder's size is the total of everything below it, as in
//! the Explorer.

use crate::bg::Latest;
use crate::catalog::{CommandSpec, RunControl};
use crate::docker::cli::DockerCli;
use crate::docker::vmtools::{self, BrowseSource};

/// One entry of a listed folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    /// Bytes of a file; 0 for a folder (a folder's size is not known).
    pub size: u64,
    /// Unix seconds, 0 when unknown (`ls` does not tell).
    pub mtime: i64,
}

/// The listing script's output: `kind TAB size TAB modified TAB name NUL`
/// per entry; the NUL ending makes any name (spaces, tabs, newlines) safe.
pub fn parse_listing(out: &str) -> Vec<FileEntry> {
    out.split('\0')
        .filter_map(|record| {
            let mut parts = record.splitn(4, '\t');
            let (kind, size, mtime, name) =
                (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            // A leading newline can come from the previous record's end.
            let name = name.to_string();
            if name.is_empty() || kind.trim().is_empty() {
                return None;
            }
            Some(FileEntry {
                name,
                is_dir: kind.trim() == "d",
                size: size.trim().parse().unwrap_or(0),
                mtime: mtime
                    .trim()
                    .split('.')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
            })
        })
        .collect()
}

/// Folders first, then by name without regard to case.
pub fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// What a finished listing carries.
pub type Listing = Result<(String, Vec<FileEntry>), String>;

/// The browser's state: where it is, what is there, and the cursor.
pub struct BrowseState {
    pub source: BrowseSource,
    /// What is being browsed, for the title: `volume pgdata`.
    pub label: String,
    /// The folder shown, absolute inside the source (`/` at the top).
    pub dir: String,
    pub all: Vec<FileEntry>,
    /// Indexes into `all` that match the filter.
    pub shown: Vec<usize>,
    pub cursor: usize,
    pub offset: usize,
    pub error: Option<String>,
    /// A listing has come back at least once (so the helper image exists).
    pub listed_once: bool,
    pub job: Latest<Listing>,
}

impl BrowseState {
    pub fn new(source: BrowseSource, label: String) -> BrowseState {
        BrowseState {
            source,
            label,
            dir: "/".to_string(),
            all: Vec::new(),
            shown: Vec::new(),
            cursor: 0,
            offset: 0,
            error: None,
            listed_once: false,
            job: Latest::new(),
        }
    }

    pub fn loading(&self) -> bool {
        self.job.pending
    }

    pub fn selected(&self) -> Option<&FileEntry> {
        self.shown.get(self.cursor).and_then(|&i| self.all.get(i))
    }

    /// The path of folder `name` inside the current one.
    pub fn child_dir(&self, name: &str) -> String {
        if self.dir == "/" {
            format!("/{name}")
        } else {
            format!("{}/{name}", self.dir)
        }
    }

    /// The parent of the current folder (`/` stays `/`).
    pub fn parent_dir(&self) -> String {
        match self.dir.rsplit_once('/') {
            Some(("", _)) | None => "/".to_string(),
            Some((parent, _)) => parent.to_string(),
        }
    }

    /// Keep only the entries matching `query`, the cursor on the same one.
    pub fn refilter(&mut self, query: &crate::filter::Query) {
        let keep = self.selected().map(|e| e.name.clone());
        self.shown = (0..self.all.len())
            .filter(|&i| query.is_empty() || query.matches(&self.all[i].name).is_some())
            .collect();
        self.cursor = keep
            .and_then(|k| self.shown.iter().position(|&i| self.all[i].name == k))
            .unwrap_or(self.cursor)
            .min(self.shown.len().saturating_sub(1));
    }

    /// List `dir` on a thread; only the newest request counts. A volume
    /// needs the helper image: it is built first when missing.
    pub fn start(&mut self, cli: &DockerCli, dir: &str, helper_ready: bool) {
        let (cli, source, dir) = (cli.clone(), self.source.clone(), dir.to_string());
        self.job.spawn(move || -> Listing {
            if matches!(source, BrowseSource::Volume(_)) && !helper_ready {
                let build = vmtools::build_spec(&cli);
                let done = cli.runner().run(&build, &RunControl::new());
                if !done.ok {
                    return Err(
                        "could not build the helper image (Docker needs to pull busybox once)"
                            .to_string(),
                    );
                }
            }
            let spec: CommandSpec = vmtools::browse_spec(&cli, &source, &dir);
            let Some(out) = cli.runner().query(&spec) else {
                return Err(match source {
                    BrowseSource::Image(_) => {
                        "this image has no shell, du or stat to list with (or Docker did not answer)"
                            .to_string()
                    }
                    BrowseSource::Volume(_) => "Docker did not answer".to_string(),
                });
            };
            let mut entries = parse_listing(&out);
            sort_entries(&mut entries);
            Ok((dir, entries))
        });
    }

    /// Take in a finished listing.
    pub fn poll(&mut self, query: &crate::filter::Query) {
        if !self.job.poll() {
            return;
        }
        match self.job.value.take() {
            Some(Ok((dir, entries))) => {
                self.dir = dir;
                self.all = entries;
                self.error = None;
                self.listed_once = true;
                self.cursor = 0;
                self.offset = 0;
                self.refilter(query);
            }
            Some(Err(e)) => self.error = Some(e),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_listing_parses_names_with_spaces_tabs_and_newlines() {
        let out =
            "f\t12\t1700000000.5000000000\tmy file.txt\0d\t4096\t1700000100.0\tweird\tname\nline\0";
        let e = parse_listing(out);
        assert_eq!(e.len(), 2);
        assert_eq!(
            e[0],
            FileEntry {
                name: "my file.txt".into(),
                is_dir: false,
                size: 12,
                mtime: 1_700_000_000
            }
        );
        assert_eq!(e[1].name, "weird\tname\nline");
        assert!(e[1].is_dir);
    }

    #[test]
    fn find_listing_skips_malformed_records_and_empty_input() {
        assert!(parse_listing("").is_empty());
        assert!(parse_listing("\0\0").is_empty());
        assert!(parse_listing("garbage\0f\tnot-a-number\t1\tname\0").len() == 1);
        let e = parse_listing("f\tnot-a-number\tnope\tname\0");
        assert_eq!((e[0].size, e[0].mtime), (0, 0), "bad numbers become zero");
        assert!(parse_listing("f\t1\t1\t\0").is_empty(), "no name");
    }

    #[test]
    fn entries_sort_folders_first_then_by_name_ignoring_case() {
        let mut e = parse_listing("f\t1\t1\tb.txt\0f\t1\t1\tA.txt\0d\t1\t1\tzdir\0d\t1\t1\tadir\0");
        sort_entries(&mut e);
        let names: Vec<&str> = e.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["adir", "zdir", "A.txt", "b.txt"]);
    }

    #[test]
    fn paths_go_down_and_up() {
        let mut s = BrowseState::new(BrowseSource::Volume("v".into()), "volume v".into());
        assert_eq!((s.dir.as_str(), s.parent_dir().as_str()), ("/", "/"));
        assert_eq!(s.child_dir("a"), "/a");
        s.dir = "/a".into();
        assert_eq!(s.child_dir("b"), "/a/b");
        assert_eq!(s.parent_dir(), "/");
        s.dir = "/a/b".into();
        assert_eq!(s.parent_dir(), "/a");
    }

    #[test]
    fn the_filter_keeps_matching_names_and_the_cursor() {
        let mut s = BrowseState::new(BrowseSource::Volume("v".into()), "volume v".into());
        s.all = parse_listing("f\t1\t1\talpha\0f\t1\t1\tbeta\0f\t1\t1\talpine\0");
        s.refilter(&crate::filter::parse(""));
        s.cursor = 2;
        s.refilter(&crate::filter::parse("alp"));
        let names: Vec<&str> = s.shown.iter().map(|&i| s.all[i].name.as_str()).collect();
        assert_eq!(names, ["alpha", "alpine"]);
        assert_eq!(
            s.selected().unwrap().name,
            "alpine",
            "stays on the same one"
        );
    }
}
