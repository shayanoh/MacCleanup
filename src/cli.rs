use std::path::{Path, PathBuf};

use clap::Parser;

/// A colorful terminal app that finds disk usage on macOS and cleans it up safely.
#[derive(Parser, Debug, Clone, Default)]
#[command(name = "maccleanup", version, about)]
pub struct Cli {
    /// Folder to explore (default: your home folder), e.g. `maccleanup .`
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Same as PATH, kept so older scripts keep working.
    #[arg(
        long = "root",
        hide = true,
        value_name = "DIR",
        conflicts_with = "path"
    )]
    pub root_flag: Option<PathBuf>,

    /// Folder that holds soft-deleted items (default: ~/ToBeDeleted).
    #[arg(long)]
    pub holding_dir: Option<PathBuf>,

    /// Use ASCII-only characters (for terminals without Nerd Font / Unicode support).
    #[arg(long)]
    pub ascii: bool,

    /// Turn off animations.
    #[arg(long)]
    pub no_animation: bool,
}

impl Cli {
    /// The folder to explore: the PATH argument (or the old `--root` flag).
    pub fn root(&self) -> Option<&Path> {
        self.path.as_deref().or(self.root_flag.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::path::Path;

    #[test]
    fn a_path_argument_is_the_root() {
        let cli = Cli::parse_from(["maccleanup", "--ascii", "--no-animation", "/tmp/x"]);
        assert!(cli.ascii);
        assert!(cli.no_animation);
        assert_eq!(cli.root(), Some(Path::new("/tmp/x")));
    }

    #[test]
    fn dot_and_relative_paths_are_accepted_as_given() {
        assert_eq!(
            Cli::parse_from(["maccleanup", "."]).root(),
            Some(Path::new("."))
        );
        assert_eq!(
            Cli::parse_from(["maccleanup", "../other"]).root(),
            Some(Path::new("../other"))
        );
    }

    #[test]
    fn the_path_can_come_before_or_after_the_flags() {
        let before = Cli::parse_from(["maccleanup", "/tmp/x", "--ascii"]);
        let after = Cli::parse_from(["maccleanup", "--ascii", "/tmp/x"]);
        assert_eq!(before.root(), after.root());
        assert!(before.ascii && after.ascii);
    }

    #[test]
    fn a_path_that_looks_like_a_flag_works_after_double_dash() {
        let cli = Cli::parse_from(["maccleanup", "--", "-weird"]);
        assert_eq!(cli.root(), Some(Path::new("-weird")));
    }

    #[test]
    fn the_old_root_flag_still_works() {
        let cli = Cli::parse_from(["maccleanup", "--root", "/tmp/x"]);
        assert_eq!(cli.root(), Some(Path::new("/tmp/x")));
    }

    #[test]
    fn a_path_and_the_root_flag_together_are_rejected() {
        assert!(Cli::try_parse_from(["maccleanup", "--root", "/a", "/b"]).is_err());
    }

    #[test]
    fn two_paths_are_rejected() {
        assert!(Cli::try_parse_from(["maccleanup", "/a", "/b"]).is_err());
    }

    #[test]
    fn defaults_are_off() {
        let cli = Cli::parse_from(["maccleanup"]);
        assert!(!cli.ascii);
        assert!(!cli.no_animation);
        assert_eq!(cli.root(), None);
        assert_eq!(cli.holding_dir, None);
    }
}
