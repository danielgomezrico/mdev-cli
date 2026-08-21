use std::path::Path;

use crate::commands::purge::common::delete_paths;
use crate::commands::purge::PurgeArgs;
use crate::logger::Logger;

pub fn run(args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();

    let explicit_flags = args.flutter || args.pub_cache || args.gradle || args.android || args.ios;
    let do_ios_pods = (!explicit_flags || args.ios) && cfg!(target_os = "macos");

    if do_ios_pods {
        delete_paths(
            &[
                root.join("Pods"),
                root.join(".symlinks"),
                root.join("build"),
            ],
            dry_run,
            verbose,
            &logger,
        );
    }
}
