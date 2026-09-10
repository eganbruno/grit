//! Command-line grammar.
//!
//! This module holds clap types and nothing else — no I/O, no logic. Adding a
//! command starts here (see CONTRIBUTING.md) and the compiler then points at
//! the one match arm in `commands::dispatch` that needs updating.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::render::ColorChoice;
use crate::shell::Shell;

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
  grit detail docs                         one repo in depth
  grit docs commit -am \"changelog\"         run git in the docs repo
  grit @release fetch                      fetch every repo tagged release
  grit -k @release fetch                   keep going when one of them fails
  grit help status                         same as `grit status --help`

Seeing the dashboard without asking for it:
  grit shell enable                        add the integration to your rc file
  eval \"$(grit shell init zsh)\"            or write that line yourself

Environment:
  GRIT_CONFIG        the registry file  (default ~/.config/grit/config.toml)
  GRIT_CACHE         the last reading   (default ~/.cache/grit/status.json)
  XDG_CONFIG_HOME    moves both of those defaults, as does XDG_CACHE_HOME
  NO_COLOR           set to anything non-empty to drop colour; see --color
  COLUMNS            table width to assume when stdout is not a terminal

Every command carries its own examples:
  grit register --help   grit status --help   grit shell enable --help
";
/// Shown under `grit shell enable --help`.
const ENABLE_HELP: &str = "\
Examples:
  grit shell enable                  the startup file for the shell $SHELL names
  grit shell enable zsh              ~/.zshrc, or $ZDOTDIR/.zshrc if that is set
  grit shell enable bash             ~/.bashrc
  grit shell enable fish             ~/.config/fish/config.fish
  grit shell enable --file ~/.zshrc.local     somewhere of your choosing

It appends one marked block, and tells you which file it touched:

  # >>> grit shell integration >>>
  eval \"$(grit shell init zsh)\"
  # <<< grit shell integration <<<

Running it twice is harmless — an existing block is left as it is rather than
added a second time. `grit shell disable` takes back exactly that block.
";

/// Shown under `grit shell disable --help`.
const DISABLE_HELP: &str = "\
Examples:
  grit shell disable                 the startup file for the shell $SHELL names
  grit shell disable zsh             name the shell yourself
  grit shell disable --file ~/.zshrc.local     the file `enable --file` wrote

Only the marked block that `enable` wrote is removed, matched by its markers.
An `eval \"$(grit shell init …)\"` line you added by hand is reported and left
alone: guessing where somebody's own line ends is how a tool eats a config.
";

/// Shown under `grit shell init --help`.
const INIT_HELP: &str = "\
Examples:
  grit shell init zsh                print the zsh integration
  eval \"$(grit shell init zsh)\"      what a startup file should contain
  grit shell init zsh > ~/.grit.zsh  keep a copy and source that instead

  grit shell init bash               ^G, since bash runs no hook while idle
  grit shell init fish               ^G, for the same reason

This only prints. It writes nothing and changes nothing; `grit shell enable`
is the one that edits a file.
";

/// Shown under `grit register --help`.
const REGISTER_HELP: &str = "\
Examples:
  grit -r api ~/code/api                    register that path as `api`
  grit -r api                               register the current directory
  grit -r api ~/code/api/src                any directory inside it will do
  grit -r api ~/code/api -t release,backend two tags at once
  grit -r api ~/code/api -t release -t web  the same, spelled out
  grit -r api ~/moved/api --force           point an existing alias elsewhere
  grit register api ~/code/api              the long form; `grit add` also works

grit stores the repository *root*, whichever directory inside it you name, and
works out whether that root is a git or a dolt repository by looking — you
never say which. A dolt database inside a git working tree registers as the
database. It has to be a directory: the path is where grit runs the detection,
so naming a file in the repo is an error rather than a shorthand for its
parent.
";

/// Shown under `grit remove --help`.
const REMOVE_HELP: &str = "\
Examples:
  grit rm api                  forget one alias
  grit rm api docs webapp      forget several at once
  grit remove api              the long form

Only grit's registry entry goes. The repository on disk is left alone.
";

/// Shown under `grit show --help`.
const SHOW_HELP: &str = "\
Examples:
  grit show                    every registered repo: alias, kind, tags, path
  grit show --tag release      just the repos tagged `release`
  grit show --json             the same, machine-readable

Reading the JSON in a script. It is an object — `repos`, plus the `config`
file the answer came from — where `grit status --json` is a bare array:
  grit show --json | jq -r '.repos[].path'
  grit show --json | jq -r '.repos[] | select(.kind == \"dolt\") | .alias'
  cd \"$(grit show --json | jq -r '.repos[] | select(.alias == \"api\") | .path')\"
";

/// Shown under `grit detail --help`.
const DETAIL_HELP: &str = "\
Examples:
  grit detail api              everything grit knows about one repo
  grit detail api --json       the same, machine-readable

What it shows, in sections, and each one only when it has something in it:
  the header      branch, upstream, sync and working-tree state
  CHANGES         one row per changed path, with a two-column code
                  left = staged, right = unstaged; `?` untracked, `U` conflicted
  COMMITS         the last few, most recent first
  BRANCHES        each branch against its upstream; `*` marks the current one
                  a blank sync column means nothing was compared — no upstream,
                  or one that has been deleted — which is not the same as `✓`
  STASHES         the stash stack, newest first

Unlike `grit status`, this runs the backend every time — there is no cache
behind it, because it is one repo and it is asked for by name.

  grit detail api --json | jq -r '.files[].path'
  grit detail api --json | jq -r '.branches[] | select(.distance.behind > 0) | .name'
";

/// Shown under `grit status --help`.
const STATUS_HELP: &str = "\
Examples:
  grit status                  every registered repo
  grit status api docs         just these two
  grit status --tag release    just the repos tagged `release`
  grit status --json           the full snapshot, machine-readable
  grit status --cached         the last reading, in milliseconds

Reading the table:
  SYNC    ✓ in sync · ↑n ahead · ↓n behind · · no upstream to compare against
  STATE   !n conflicts · ●n staged · ○n unstaged · ?n untracked · ⚑n stashes
          `clean` when there is nothing to report, and `missing` when the
          registered path is gone. An in-progress merge, rebase, cherry-pick,
          revert or bisect is named here instead.

--cached runs no git and no dolt at all, which is what makes it cheap enough
for a shell prompt or a tmux status line. It prints nothing and exits non-zero
when there is no reading yet, so a script can tell that apart from an empty
registry:

  grit status --cached || grit status     show the cache, or go and take one

Only an unfiltered run refills that cache. A run narrowed by alias or by --tag
leaves it untouched, so a partial reading can never pass itself off as the
whole dashboard — which would read as `clean` rather than as `not looked at`.
";

/// Shown under `grit shell --help`.
const SHELL_HELP: &str = "\
Examples:
  grit shell enable                     add it to the startup file for $SHELL
  grit shell enable zsh                 name the shell yourself
  grit shell enable --file ~/.zshrc     write to a particular file
  grit shell disable                    take back exactly the block it wrote
  grit shell init zsh                   print the integration, to eval yourself

grit edits a startup file when you ask it to and at no other time — installing
grit changes nothing on its own. `disable` removes only the marked block that
`enable` wrote; a line you added yourself is reported rather than edited.

zsh draws the table after a pause. bash and fish bind a key instead, because
neither runs a hook while you sit at the prompt.

Keys, in all three shells:
  ^G^G    the repo picker: every repo, with `grit detail` beside it. Needs fzf.
  ^G^P    the inline dashboard, on demand

Both are chords, and nothing is bound to ^G on its own. A line editor resolves
an ambiguous prefix by waiting for the next key — 404ms in zsh, 504ms in
readline — and charges it to the shorter binding, so anything left on a bare ^G
pauses before it fires, every press. The letters also stay clear of
fzf-git.sh's ^G^{f,b,t,r,h,s,l,e,w}, which is common and claims ^G too.

GRIT_PREVIEW_KEY='^G' still works if you want the old single key. It will just
pause. grit binds what you ask for and does not rearrange it.

Inside the picker:
  enter   put `grit <alias> ` on the command line, for you to finish
  ctrl-d  cd to the repo
  ctrl-r  take a fresh reading

Tuning, set *before* the eval:
  GRIT_PREVIEW_TRIGGERS=( grit gs )     buffers that summon it   (grit)
  GRIT_PREVIEW_DELAY=0.2                seconds of stillness     (0.5)
  GRIT_PREVIEW_KEY='^T'                 draw it on demand      (^G^P)
  GRIT_PREVIEW_IDLE=0                   the key only, no timer
  GRIT_PICKER_KEY='^T^R'                open the picker        (^G^G)
  GRIT_PICKER_HEIGHT=100%               how much screen it takes (80%)
  GRIT_PICKER_PREVIEW=down,60%          where the detail pane goes

Before, because the keys are bound as the script is sourced — set them
afterwards and the binding is already made. The rest are read as you type, so
they do take effect later; setting everything up front is the rule that holds.
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
    ///
    /// Deliberately not `global`. Only the fan-out reads it, and a global flag
    /// is listed in every subcommand's help, so `grit show --help` used to
    /// advertise "keep going after a repo fails" on a command that has no
    /// fan-out to keep going with. `--color` is global because every command
    /// paints; this one is not.
    #[arg(short = 'k', long)]
    pub keep_going: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Register a repository under an alias.
    #[command(visible_alias = "add", after_help = REGISTER_HELP)]
    Register(RegisterArgs),

    /// Forget a registered repository. The repository itself is untouched.
    #[command(visible_alias = "rm", after_help = REMOVE_HELP)]
    Remove(RemoveArgs),

    /// Show every registered repository with its path and tags.
    #[command(after_help = SHOW_HELP)]
    Show(ShowArgs),

    /// Dashboard: branch, sync state and working-tree state for each repo.
    #[command(after_help = STATUS_HELP)]
    Status(StatusArgs),

    /// One repo in depth: changed paths, recent commits, branches, stashes.
    #[command(after_help = DETAIL_HELP)]
    Detail(DetailArgs),

    /// Shell integration — the dashboard, at the prompt, before you hit enter.
    #[command(after_help = SHELL_HELP)]
    Shell(ShellArgs),

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

    /// Path to the repo. Any directory inside it works; grit stores the root.
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

    /// Show the last reading instead of taking a new one.
    ///
    /// Runs no git and no dolt, so it answers in milliseconds — which is what
    /// makes a dashboard cheap enough to put in a prompt, a tmux status line or
    /// the shell integration. The footer says how old the reading is.
    #[arg(long)]
    pub cached: bool,
}

#[derive(Debug, Args)]
pub struct DetailArgs {
    /// The repo to look at.
    pub alias: String,

    /// Emit JSON instead of the card.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ShellArgs {
    #[command(subcommand)]
    pub command: ShellCommand,
}

/// `enable`, `disable` and `init` are the parts anyone types. The other two are
/// the protocol the emitted script speaks back to grit, and are hidden because a
/// human has no use for them — but they are ordinary commands, not a private
/// channel, so a curious user running one gets something sensible rather than a
/// panic.
#[derive(Debug, Subcommand)]
pub enum ShellCommand {
    /// Add the integration to your shell's startup file.
    #[command(after_help = ENABLE_HELP)]
    Enable(ShellSetupArgs),

    /// Take the integration back out of your shell's startup file.
    #[command(after_help = DISABLE_HELP)]
    Disable(ShellSetupArgs),

    /// Print the integration for a shell. Feed it to `eval` from your rc file.
    #[command(after_help = INIT_HELP)]
    Init(ShellInitArgs),

    /// The cached dashboard as plain text plus the ranges to colour.
    #[command(hide = true)]
    Preview(ShellPreviewArgs),

    /// Take a fresh reading into the cache, printing nothing.
    #[command(hide = true)]
    Refresh(ShellRefreshArgs),

    /// The dashboard's rows alone, one repo per line, for a picker to read.
    #[command(hide = true)]
    Rows(ShellRowsArgs),

    /// Where one alias points, so a picker can cd there.
    #[command(hide = true)]
    Path(ShellPathArgs),
}

#[derive(Debug, Args)]
pub struct ShellInitArgs {
    #[arg(value_enum)]
    pub shell: Shell,
}

#[derive(Debug, Args)]
pub struct ShellSetupArgs {
    /// Which shell. Defaults to the one `$SHELL` names.
    #[arg(value_enum)]
    pub shell: Option<Shell>,

    /// Edit this file instead of the shell's usual startup file.
    ///
    /// What `GRIT_CONFIG` is for the registry: a way to exercise this without
    /// writing to the startup file of whoever is running the tests.
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ShellPreviewArgs {
    /// Show at most this many repos, and say how many were left out.
    ///
    /// The preview is drawn *below* the line being typed, so a table taller
    /// than the screen scrolls the prompt away — the shell passes the room it
    /// actually has.
    #[arg(long, value_name = "N")]
    pub max_rows: Option<usize>,

    /// Draw nothing if the last reading is older than this many seconds.
    ///
    /// A table that appeared on its own is read at a glance, so showing a
    /// four-hour-old one is worse than showing none: the caller has just
    /// started a refresh and will ask again a second later.
    #[arg(long, value_name = "SECONDS")]
    pub max_age: Option<i64>,
}

#[derive(Debug, Args)]
pub struct ShellPathArgs {
    /// The repo to locate.
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct ShellRowsArgs {
    /// Show the last reading rather than taking a new one.
    ///
    /// The picker opens on the cache so it appears instantly, exactly as the
    /// inline preview does, and refreshes behind itself.
    #[arg(long)]
    pub cached: bool,
}

#[derive(Debug, Args)]
pub struct ShellRefreshArgs {
    /// Do nothing if the cache is younger than this many seconds.
    ///
    /// The integration fires a refresh on every idle tick; this is what stops
    /// that from meaning a `git status` per second per repository.
    #[arg(long, value_name = "SECONDS", default_value_t = 0)]
    pub max_age: i64,
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

    /// The tool documents itself: `grit <command> --help` is where the worked
    /// examples live, so a command added without any is a documentation
    /// regression and not a style nit. Hidden commands are exempt — they are
    /// the protocol the shell script speaks, not something anyone types.
    #[test]
    fn every_visible_command_carries_examples() {
        fn walk(cmd: &clap::Command, path: &str, missing: &mut Vec<String>) {
            for sub in cmd.get_subcommands() {
                if sub.is_hide_set() || sub.get_name() == "help" {
                    continue;
                }
                let path = format!("{path} {}", sub.get_name());
                let documented = sub
                    .get_after_help()
                    .is_some_and(|help| help.to_string().contains("Examples:"));
                if !documented {
                    missing.push(path.clone());
                }
                walk(sub, &path, missing);
            }
        }

        let root = Cli::command();
        assert!(
            root.get_after_help()
                .is_some_and(|help| help.to_string().contains("Examples:")),
            "`grit --help` carries no examples"
        );

        let mut missing = Vec::new();
        walk(&root, "grit", &mut missing);
        assert!(
            missing.is_empty(),
            "no `Examples:` in the help for: {missing:?}"
        );
    }

    /// `ENABLE_HELP` shows the block `shell enable` writes, markers and all.
    /// That is a copy of two constants, and the whole property of a copy is
    /// that it can drift — leaving the help confidently describing a block
    /// `disable` would no longer recognise.
    #[test]
    fn the_enable_help_quotes_the_real_block_markers() {
        for marker in [crate::shell::BEGIN, crate::shell::END] {
            assert!(
                ENABLE_HELP.contains(marker),
                "`shell enable` help does not show the marker it writes: {marker}"
            );
        }
    }

    /// Every environment variable the root help advertises has to be one grit
    /// actually reads. The two `GRIT_*` names are consts; the rest are read
    /// inline, so this at least pins the spelling against a typo.
    #[test]
    fn the_advertised_environment_variables_are_the_real_ones() {
        for name in [
            crate::registry::CONFIG_ENV,
            crate::cache::CACHE_ENV,
            "XDG_CONFIG_HOME",
            "NO_COLOR",
            "COLUMNS",
        ] {
            assert!(
                AFTER_HELP.contains(name),
                "`grit --help` does not mention {name}"
            );
        }
    }

    /// `-k` is read only by the group fan-out. It used to be `global`, which
    /// makes clap offer it on every subcommand's help — so `grit show --help`
    /// advertised "keep going after a repo fails" on a command with no repos
    /// to keep going over, and `grit show -k` exited 0 having done nothing.
    #[test]
    fn keep_going_belongs_to_the_fan_out_and_not_to_every_command() {
        assert!(
            Cli::try_parse_from(normalize_args(["grit", "-k", "@release", "fetch"])).is_ok(),
            "the form the docs advertise has to keep working"
        );

        for args in [
            ["grit", "show", "-k"].as_slice(),
            ["grit", "status", "-k"].as_slice(),
            ["grit", "shell", "init", "zsh", "-k"].as_slice(),
        ] {
            assert!(
                Cli::try_parse_from(normalize_args(args.to_vec())).is_err(),
                "{args:?} should be a usage error, not a silent no-op"
            );
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
