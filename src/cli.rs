//! Command-line grammar.
//!
//! This module holds clap types and nothing else — no I/O, no logic. Adding a
//! command starts here (see CONTRIBUTING.md) and the compiler then points at
//! the one match arm in `commands::dispatch` that needs updating.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::render::ColorChoice;

/// Shown under the generated help.
///
/// The passthrough forms live here because clap cannot list an external
/// subcommand under `Commands:`, and they are the point of the tool.
const AFTER_HELP: &str = "\
Running git or dolt in a repo:
  grit <alias> <args...>   run the repo's own VCS there, from anywhere
  grit @<tag> <args...>    run it in every repo carrying that tag

Examples:
  grit -r docs ~/code/docs --tag release   register a repo under an alias
  grit status                              dashboard for every repo
  grit status --tag release                just the release group
  grit show                                aliases, paths and tags
  grit docs commit -am \"changelog\"         run git in the docs repo
  grit @release fetch                      fetch every repo tagged release
";

#[derive(Debug, Parser)]
#[command(
    name = "grit",
    version,
    about = "Work across many repositories from anywhere, by alias.",
    after_help = AFTER_HELP,
    disable_help_subcommand = true,
    subcommand_negates_reqs = true
)]
pub struct Cli {
    #[arg(
        long,
        value_enum,
        default_value_t = ColorChoice::Auto,
        global = true,
        value_name = "WHEN",
        help = "When to colourise output"
    )]
    pub color: ColorChoice,

    /// Keep going after a repo fails during a group fan-out.
    ///
    /// Must precede the target — everything after `grit @release` belongs to
    /// the command being run.
    #[arg(short = 'k', long, global = true)]
    pub keep_going: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Register a repository under an alias.
    #[command(visible_alias = "add")]
    Register(RegisterArgs),

    /// Forget a registered repository. The repository itself is untouched.
    #[command(visible_alias = "rm")]
    Remove(RemoveArgs),

    /// Show every registered repository with its path and tags.
    Show(ShowArgs),

    /// Dashboard: branch, sync state and working-tree state for each repo.
    Status(StatusArgs),

    /// `grit <alias|@tag> <args...>` — run the repo's VCS with those arguments.
    ///
    /// Captured by clap as an external subcommand, so flags meant for git are
    /// passed through untouched rather than being parsed by grit.
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[derive(Debug, Args)]
pub struct RegisterArgs {
    /// Short name to refer to the repo by.
    pub alias: String,

    /// Path to the repo. Any path inside it works; grit stores the root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Group label. Repeatable, or comma-separated.
    #[arg(short, long = "tag", value_name = "TAG", value_delimiter = ',')]
    pub tags: Vec<String>,

    /// Overwrite the alias if it is already registered.
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// Aliases to forget.
    #[arg(required = true)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Only show repos carrying this tag.
    #[arg(short, long, value_name = "TAG")]
    pub tag: Option<String>,

    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Only these aliases. Default is every registered repo.
    pub aliases: Vec<String>,

    /// Only repos carrying this tag.
    #[arg(short, long, value_name = "TAG")]
    pub tag: Option<String>,

    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// The two spellings of the register shorthand.
const REGISTER_SHORTHAND: [&str; 2] = ["-r", "--register"];

/// Rewrite `grit -r ...` into `grit register ...`.
///
/// `-r` is the shorthand the tool advertises, but a flag that swallows two
/// values cannot also accept `register`'s own flags (`--tag`, `--force`).
/// Rewriting the token gives the shorthand full parity with the subcommand for
/// the cost of one scan, and keeps the clap grammar free of contortions.
///
/// The scan stops at the first token that is not a top-level flag — which is
/// the subcommand or passthrough target. So `grit --color never -r a /p` is
/// rewritten, while `grit docs push -r` reaches git untouched.
pub fn normalize_args<I, T>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let flags = top_level_flags();

    let mut i = 1;
    while i < args.len() {
        let Some(arg) = args[i].to_str() else { break };

        if REGISTER_SHORTHAND.contains(&arg) {
            args[i] = OsString::from("register");
            break;
        }

        // `--color=always` carries its value inline; `--color always` does not.
        match arg.split_once('=') {
            Some((name, _)) if flags.iter().any(|(n, _)| n == name) => i += 1,
            _ => match flags.iter().find(|(name, _)| name == arg) {
                Some((_, values)) => i += 1 + values,
                None => break,
            },
        }
    }

    args
}

/// Every top-level flag, with how many values it consumes.
///
/// Read back off the clap grammar rather than hardcoded, so adding a global
/// flag cannot silently break the `-r` rewrite.
fn top_level_flags() -> Vec<(String, usize)> {
    use clap::CommandFactory as _;

    // `build()` fills in the value counts the derive left implicit; without it
    // `--color` would look like a flag that takes no value.
    let mut cmd = Cli::command();
    cmd.build();

    cmd.get_arguments()
        .flat_map(|arg| {
            let values = arg.get_num_args().map_or_else(
                || usize::from(arg.get_action().takes_values()),
                |range| range.min_values(),
            );
            let long = arg.get_long().map(|l| format!("--{l}"));
            let short = arg.get_short().map(|s| format!("-{s}"));
            [long, short]
                .into_iter()
                .flatten()
                .map(move |name| (name, values))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(normalize_args(args)).expect("should parse")
    }

    #[test]
    fn the_grammar_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    /// Guards the invariant behind `RESERVED_ALIASES`: a new command must not
    /// be shadowable by an alias someone registered earlier.
    #[test]
    fn every_subcommand_is_reserved() {
        use crate::registry::RESERVED_ALIASES;

        for sub in Cli::command().get_subcommands() {
            let name = sub.get_name();
            if name == "help" {
                continue;
            }
            assert!(
                RESERVED_ALIASES.contains(&name),
                "subcommand `{name}` is missing from RESERVED_ALIASES"
            );
            for alias in sub.get_all_aliases() {
                assert!(
                    RESERVED_ALIASES.contains(&alias),
                    "alias `{alias}` of `{name}` is missing from RESERVED_ALIASES"
                );
            }
        }
    }

    #[test]
    fn dash_r_is_shorthand_for_register() {
        let cli = parse(&["grit", "-r", "docs", "/code/docs"]);
        let Some(Command::Register(args)) = cli.command else {
            panic!("expected register, got {:?}", cli.command);
        };
        assert_eq!(args.alias, "docs");
        assert_eq!(args.path, PathBuf::from("/code/docs"));
    }

    #[test]
    fn the_shorthand_accepts_registers_own_flags() {
        let cli = parse(&["grit", "-r", "api", "/code/edge", "--tag", "release", "-f"]);
        let Some(Command::Register(args)) = cli.command else {
            panic!("expected register");
        };
        assert_eq!(args.tags, ["release"]);
        assert!(args.force);
    }

    #[test]
    fn tags_may_be_repeated_or_comma_separated() {
        let repeated = parse(&["grit", "register", "a", ".", "-t", "x", "-t", "y"]);
        let comma = parse(&["grit", "register", "a", ".", "-t", "x,y"]);
        for cli in [repeated, comma] {
            let Some(Command::Register(args)) = cli.command else {
                panic!("expected register");
            };
            assert_eq!(args.tags, ["x", "y"]);
        }
    }

    #[test]
    fn register_defaults_to_the_current_directory() {
        let cli = parse(&["grit", "-r", "here"]);
        let Some(Command::Register(args)) = cli.command else {
            panic!("expected register");
        };
        assert_eq!(args.path, PathBuf::from("."));
    }

    #[test]
    fn an_unknown_first_word_becomes_a_passthrough() {
        let cli = parse(&["grit", "docs", "commit", "-am", "changelog"]);
        let Some(Command::External(args)) = cli.command else {
            panic!("expected external, got {:?}", cli.command);
        };
        assert_eq!(args, ["docs", "commit", "-am", "changelog"]);
    }

    #[test]
    fn passthrough_does_not_swallow_git_flags() {
        let cli = parse(&["grit", "api", "log", "--oneline", "-n", "5"]);
        let Some(Command::External(args)) = cli.command else {
            panic!("expected external");
        };
        assert_eq!(args, ["api", "log", "--oneline", "-n", "5"]);
    }

    #[test]
    fn a_group_target_reaches_the_passthrough_intact() {
        let cli = parse(&["grit", "@release", "fetch"]);
        let Some(Command::External(args)) = cli.command else {
            panic!("expected external");
        };
        assert_eq!(args, ["@release", "fetch"]);
    }

    #[test]
    fn the_shorthand_works_after_global_flags() {
        for prefix in [
            vec!["grit", "--color", "never"],
            vec!["grit", "--color=never"],
            vec!["grit", "-k"],
        ] {
            let mut argv = prefix.clone();
            argv.extend(["-r", "docs", "/code/docs"]);
            let cli = parse(&argv);
            assert!(
                matches!(cli.command, Some(Command::Register(_))),
                "{prefix:?} then -r gave {:?}",
                cli.command
            );
        }
    }

    #[test]
    fn a_trailing_dash_r_is_left_for_git() {
        let cli = parse(&["grit", "docs", "push", "-r"]);
        let Some(Command::External(args)) = cli.command else {
            panic!("expected external");
        };
        assert_eq!(args, ["docs", "push", "-r"]);
    }

    #[test]
    fn status_takes_alias_and_tag_filters() {
        let cli = parse(&[
            "grit", "status", "api", "docs", "--tag", "release", "--json",
        ]);
        let Some(Command::Status(args)) = cli.command else {
            panic!("expected status");
        };
        assert_eq!(args.aliases, ["api", "docs"]);
        assert_eq!(args.tag.as_deref(), Some("release"));
        assert!(args.json);
    }

    #[test]
    fn no_arguments_selects_no_command() {
        assert!(parse(&["grit"]).command.is_none());
    }

    #[test]
    fn colour_can_be_forced_either_way() {
        assert_eq!(
            parse(&["grit", "--color", "never"]).color,
            ColorChoice::Never
        );
        assert_eq!(
            parse(&["grit", "--color", "always", "status"]).color,
            ColorChoice::Always
        );
    }

    #[test]
    fn normalize_leaves_an_empty_argv_alone() {
        assert_eq!(normalize_args(["grit"]), [OsString::from("grit")]);
        assert!(normalize_args(Vec::<OsString>::new()).is_empty());
    }
}
