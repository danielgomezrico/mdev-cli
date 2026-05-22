use colored::Colorize;
use std::path::Path;

use crate::commands::purge::PurgeArgs;
use crate::commands::purge::common::delete_paths;
use crate::logger::Logger;
use crate::runner::{ProcessRunner, Runner};

pub fn run(args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();
    let runner = ProcessRunner::new();

    let explicit_flags = args.flutter || args.pub_cache || args.gradle || args.android || args.ios;

    let do_flutter_clean = !explicit_flags || args.flutter;
    let do_android_build = !explicit_flags || args.android || args.flutter;
    let do_ios_pods =
        (!explicit_flags || args.ios || args.flutter) && cfg!(target_os = "macos");

    if do_flutter_clean {
        run_flutter_clean(root, dry_run, verbose, &runner, &logger);
    }
    if do_android_build {
        delete_paths(
            &[
                root.join("android").join("build"),
                root.join("android").join("app").join("build"),
                root.join("android").join(".gradle"),
            ],
            dry_run,
            verbose,
            &logger,
        );
    }
    if do_ios_pods {
        delete_paths(
            &[
                root.join("ios").join("Pods"),
                root.join("ios").join(".symlinks"),
                root.join("ios").join("build"),
            ],
            dry_run,
            verbose,
            &logger,
        );
    }
}

fn run_flutter_clean(
    root: &Path,
    dry_run: bool,
    verbose: bool,
    runner: &dyn Runner,
    logger: &Logger,
) {
    let label = format!("flutter clean ({})", root.display());
    if dry_run {
        logger.detail(&format!("  {} {}", "~".cyan(), label));
        return;
    }
    let root_str = root.to_string_lossy().into_owned();
    let result = runner.run("flutter", &["clean"], Some(root_str.as_str()));
    if result.is_success() {
        logger.success(&format!("  {} {}", "✓".green(), label));
    } else {
        logger.err(&format!("  {} Failed: {}", "✗".red(), label));
        if verbose {
            logger.err(&result.stderr);
        }
    }
}
