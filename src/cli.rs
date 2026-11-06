use std::path::PathBuf;

use clap::Parser;

/// A colorful terminal app that finds disk usage on macOS and cleans it up safely.
#[derive(Parser, Debug, Clone, Default)]
#[command(name = "maccleanup", version, about)]
pub struct Cli {
    /// Folder to explore (default: your home folder).
    #[arg(long)]
    pub root: Option<PathBuf>,

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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_flags() {
        let cli = Cli::parse_from([
            "maccleanup",
            "--ascii",
            "--no-animation",
            "--root",
            "/tmp/x",
        ]);
        assert!(cli.ascii);
        assert!(cli.no_animation);
        assert_eq!(cli.root, Some(std::path::PathBuf::from("/tmp/x")));
    }

    #[test]
    fn defaults_are_off() {
        let cli = Cli::parse_from(["maccleanup"]);
        assert!(!cli.ascii);
        assert!(!cli.no_animation);
        assert_eq!(cli.root, None);
        assert_eq!(cli.holding_dir, None);
    }
}
